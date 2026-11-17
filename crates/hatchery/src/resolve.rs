//! The resolver: picks a version of every package a request needs, from
//! the sources it is given, without downloading any egg.
//!
//! Candidates are tried newest first. Constraints accumulate as packages
//! are assigned, installed packages that satisfy them are kept, and the
//! ranges of installed dependents hold the result back. A candidate that
//! fails a filter (withdrawn, requirements, variants, build, minimum
//! release age) is skipped and an older one tried.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use semver::{Version, VersionReq};

use crate::error::{At, Error, Result};
use crate::fetch::BoxFuture;
use crate::name::Name;
use crate::platform::Variants;
use crate::seen::Seen;
use crate::source::{Details, Offer, Source, TargetInfo};
use crate::target::{self, Target};
use crate::time::Timestamp;

/// Where a source's packages are installed from, as the lock file records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A registry, static or git, found by its label.
    Registry,
    Git {
        url: String,
        path: Option<String>,
        git_ref: Option<String>,
    },
    File {
        location: String,
    },
    Folder {
        path: String,
    },
}

/// One source the resolver may use.
#[derive(Clone)]
pub struct SourceEntry {
    pub source: Arc<dyn Source>,
    pub origin: Origin,
    /// The groups it may serve; any when empty.
    pub only: Vec<String>,
    /// How long a new version is held. Zero holds nothing.
    pub min_age: Duration,
}

impl SourceEntry {
    /// A registry that is searched in order and subject to the minimum
    /// release age.
    pub fn registry(source: Arc<dyn Source>, only: Vec<String>, min_age: Duration) -> Self {
        SourceEntry {
            source,
            origin: Origin::Registry,
            only,
            min_age,
        }
    }

    /// A source for one chosen package: never held, never searched.
    pub fn single(source: Arc<dyn Source>, origin: Origin) -> Self {
        SourceEntry {
            source,
            origin,
            only: Vec::new(),
            min_age: Duration::ZERO,
        }
    }

    pub fn is_registry(&self) -> bool {
        self.origin == Origin::Registry
    }

    pub fn serves(&self, group: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|g| g == group)
    }
}

/// What the resolver needs to know of an installed package.
#[derive(Debug, Clone)]
pub struct InstalledPkg {
    pub version: Version,
    /// What its installed targets require.
    pub dependencies: Vec<(Name, VersionReq)>,
}

/// A version the resolver chose to install.
#[derive(Debug, Clone)]
pub struct Pick {
    /// The index of its source.
    pub source: usize,
    pub offer: Offer,
    pub details: Details,
    /// The targets of the client the package has, which are installed.
    pub targets: Vec<String>,
}

/// The result of a resolution.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// What has to be installed.
    pub picks: BTreeMap<Name, Pick>,
    /// Installed packages that stay as they are, with their versions.
    pub kept: BTreeMap<Name, Version>,
}

type Deps = Vec<(Name, VersionReq)>;

/// What a package is asked to satisfy: who asks, and the range.
pub type Constraints = BTreeMap<Name, Vec<(String, VersionReq)>>;

#[derive(Clone)]
struct Selected {
    version: Version,
    deps: Vec<(Name, VersionReq)>,
    optional: Vec<(Name, VersionReq)>,
    pick: Option<Pick>,
}

type Assigned = BTreeMap<Name, Selected>;

#[derive(Default)]
struct State {
    seen: Seen,
    listings: BTreeMap<(usize, Name), Arc<Vec<Offer>>>,
    details: BTreeMap<(usize, Name, Version), Details>,
}

pub struct Resolver {
    sources: Vec<SourceEntry>,
    targets: Vec<Arc<dyn Target>>,
    installed: BTreeMap<Name, InstalledPkg>,
    pinned: BTreeMap<Name, usize>,
    force: BTreeSet<Name>,
    allow_young: BTreeSet<Name>,
    now: Timestamp,
    state: Mutex<State>,
}

pub(crate) fn at_of(offer: &Offer) -> At {
    At::new(&offer.name)
        .version(&offer.version)
        .registry(&offer.registry)
}

fn conflict(name: &Name, version: &Version, message: String) -> Error {
    Error::Conflict {
        at: At::new(name).version(version),
        message,
    }
}

fn range_text(reqs: &[(String, VersionReq)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (_, r) in reqs {
        let s = r.to_string();
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join(", ")
}

impl Resolver {
    /// `sources` are the registries in search order; sources added later
    /// are for single packages. `now` and `seen` serve the minimum release
    /// age.
    pub fn new(
        sources: Vec<SourceEntry>,
        targets: Vec<Arc<dyn Target>>,
        now: Timestamp,
        seen: Seen,
    ) -> Resolver {
        Resolver {
            sources,
            targets,
            installed: BTreeMap::new(),
            pinned: BTreeMap::new(),
            force: BTreeSet::new(),
            allow_young: BTreeSet::new(),
            now,
            state: Mutex::new(State {
                seen,
                ..State::default()
            }),
        }
    }

    pub fn set_installed(&mut self, installed: BTreeMap<Name, InstalledPkg>) {
        self.installed = installed;
    }

    /// Adds a source and returns its index.
    pub fn add_source(&mut self, entry: SourceEntry) -> usize {
        self.sources.push(entry);
        self.sources.len() - 1
    }

    /// Serves a package from one source, whatever the search order says.
    pub fn pin(&mut self, name: Name, source: usize) {
        self.pinned.insert(name, source);
    }

    /// A package that is looked up in its source even if the installed
    /// version satisfies the constraints.
    pub fn force(&mut self, name: Name) {
        self.force.insert(name);
    }

    /// A package that is installed even if younger than the minimum age.
    pub fn allow_young(&mut self, name: Name) {
        self.allow_young.insert(name);
    }

    pub fn source(&self, idx: usize) -> &SourceEntry {
        &self.sources[idx]
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the first-seen records to an install folder if they changed.
    pub fn save_seen(&self, dir: &std::path::Path) -> Result<()> {
        self.state().seen.save(dir)
    }

    /// The only group that has a package of this id in the registries.
    pub async fn find_name(&self, id: &str) -> Result<Name> {
        let mut groups = BTreeSet::new();
        for e in self.sources.iter().filter(|e| e.is_registry()) {
            for n in e.source.names().await? {
                if n.id == id && e.serves(&n.group) {
                    groups.insert(n.group);
                }
            }
        }
        let mut groups: Vec<String> = groups.into_iter().collect();
        match groups.len() {
            0 => Err(Error::NotFound { at: At::new(id) }),
            1 => Ok(Name {
                group: groups.remove(0),
                id: id.to_string(),
            }),
            _ => Err(Error::Ambiguous {
                id: id.to_string(),
                groups,
            }),
        }
    }

    /// The versions one source offers of a package, newest first. Every
    /// listing is recorded in the first-seen records.
    pub async fn listing(&self, idx: usize, name: &Name) -> Result<Arc<Vec<Offer>>> {
        if let Some(l) = self.state().listings.get(&(idx, name.clone())) {
            return Ok(l.clone());
        }
        let entry = &self.sources[idx];
        let mut offers = entry.source.versions(name).await?;
        offers.sort_by(|a, b| b.version.cmp(&a.version));
        let offers = Arc::new(offers);
        let mut st = self.state();
        if entry.is_registry() {
            st.seen.record(
                entry.source.label(),
                name,
                offers.iter().map(|o| &o.version),
                self.now,
            );
        }
        st.listings.insert((idx, name.clone()), offers.clone());
        Ok(offers)
    }

    /// The source that serves a package, and its versions newest first.
    pub async fn locate(&self, name: &Name) -> Result<Option<(usize, Arc<Vec<Offer>>)>> {
        if let Some(&idx) = self.pinned.get(name) {
            let offers = self.listing(idx, name).await?;
            return Ok((!offers.is_empty()).then_some((idx, offers)));
        }
        for (idx, e) in self.sources.iter().enumerate() {
            if !e.is_registry() || !e.serves(&name.group) {
                continue;
            }
            let offers = self.listing(idx, name).await?;
            if !offers.is_empty() {
                return Ok(Some((idx, offers)));
            }
        }
        Ok(None)
    }

    /// The targets and display data of an offer.
    pub async fn details(&self, idx: usize, offer: &Offer) -> Result<Details> {
        if let Some(d) = &offer.details {
            return Ok(d.clone());
        }
        let key = (idx, offer.name.clone(), offer.version.clone());
        if let Some(d) = self.state().details.get(&key) {
            return Ok(d.clone());
        }
        let d = self.sources[idx].source.details(offer).await?;
        self.state().details.insert(key, d.clone());
        Ok(d)
    }

    /// When a version becomes installable. None when nothing holds it. The
    /// time may be in the past.
    pub fn held_until(&self, idx: usize, offer: &Offer) -> Option<Timestamp> {
        let e = &self.sources[idx];
        if !e.is_registry() {
            return None;
        }
        self.state().seen.held_until(
            e.source.label(),
            &offer.name,
            &offer.version,
            offer.published,
            e.min_age,
            self.now,
        )
    }

    fn young(&self, idx: usize, offer: &Offer) -> Option<Timestamp> {
        // The version already installed was allowed once; holding it now
        // would make an update fail where keeping it is the answer.
        if self.allow_young.contains(&offer.name)
            || self
                .installed
                .get(&offer.name)
                .is_some_and(|i| i.version == offer.version)
        {
            return None;
        }
        self.held_until(idx, offer).filter(|u| *u > self.now)
    }

    /// Whether the package can be installed for this client's targets:
    /// requirements, variants and build. Returns the names of the targets
    /// that would be installed, or the reason.
    pub fn check(
        &self,
        offer: &Offer,
        details: &Details,
    ) -> std::result::Result<Vec<String>, Error> {
        let at = at_of(offer);
        let mine: Vec<(&Arc<dyn Target>, &TargetInfo)> = self
            .targets
            .iter()
            .filter_map(|t| details.target(t.name()).map(|i| (t, i)))
            .collect();
        if mine.is_empty() {
            let wanted: Vec<&str> = self.targets.iter().map(|t| t.name()).collect();
            return Err(Error::invalid(
                &at,
                format!("has no target for {}", wanted.join(", ")),
            ));
        }
        for (t, info) in &mine {
            if let Some((req, found)) = target::unmet(t.as_ref(), &info.requires) {
                return Err(Error::Unmet {
                    at,
                    target: t.name().to_string(),
                    fact: req.fact.clone(),
                    wanted: req.values.clone(),
                    found,
                });
            }
        }
        for (t, info) in &mine {
            if let Variants::Matching(p) = t.variants()
                && !info.variants.is_empty()
                && crate::manifest::select_variant(
                    &info.variants,
                    |v| (v.os.as_deref(), v.arch.as_deref()),
                    &p,
                )
                .is_none()
            {
                return Err(Error::NoVariant {
                    at,
                    target: t.name().to_string(),
                    platform: p,
                });
            }
        }
        if !offer.built {
            let building: Vec<String> = mine
                .iter()
                .filter(|(_, i)| i.build)
                .map(|(t, _)| t.name().to_string())
                .collect();
            if !building.is_empty() {
                return Err(Error::NotBuilt {
                    at,
                    targets: building,
                });
            }
        }
        Ok(mine.iter().map(|(t, _)| t.name().to_string()).collect())
    }

    /// Runs every filter on one offer.
    async fn evaluate(&self, idx: usize, offer: &Offer) -> std::result::Result<Pick, Error> {
        if let Some(w) = &offer.withdrawn {
            return Err(Error::Withdrawn {
                at: at_of(offer),
                reason: w.reason.clone(),
            });
        }
        let details = self.details(idx, offer).await?;
        let targets = self.check(offer, &details)?;
        if let Some(until) = self.young(idx, offer) {
            return Err(Error::TooYoung {
                at: at_of(offer),
                until,
            });
        }
        Ok(Pick {
            source: idx,
            offer: offer.clone(),
            details,
            targets,
        })
    }

    /// The newest version of a package in a range that is not withdrawn
    /// and not held, with no regard to requirements or variants. For
    /// fetching eggs without installing them.
    pub async fn newest(&self, name: &Name, range: &VersionReq) -> Result<(usize, Offer)> {
        let Some((idx, offers)) = self.locate(name).await? else {
            return Err(Error::NotFound {
                at: At::new(name).version(range),
            });
        };
        let mut first_skip = None;
        for offer in offers.iter().filter(|o| range.matches(&o.version)) {
            if let Some(w) = &offer.withdrawn {
                first_skip.get_or_insert(Error::Withdrawn {
                    at: at_of(offer),
                    reason: w.reason.clone(),
                });
                continue;
            }
            if let Some(until) = self.young(idx, offer) {
                first_skip.get_or_insert(Error::TooYoung {
                    at: at_of(offer),
                    until,
                });
                continue;
            }
            return Ok((idx, offer.clone()));
        }
        Err(first_skip.unwrap_or(Error::NotFound {
            at: At::new(name).version(range),
        }))
    }

    /// Resolves the roots and their dependencies. Optional dependencies of
    /// the result are then tried one at a time and kept when the solution
    /// still holds.
    pub async fn resolve(&self, roots: Constraints) -> Result<Plan> {
        let mut roots = roots;
        let mut last = None;
        let mut assigned = match self.solve_once(&roots, &mut last).await? {
            Some(a) => a,
            None => {
                return Err(last.unwrap_or_else(|| Error::Conflict {
                    at: At::new("dependencies"),
                    message: "no set of versions satisfies every range".to_string(),
                }));
            }
        };

        let mut tried: BTreeSet<(Name, Name)> = BTreeSet::new();
        loop {
            let next = assigned.iter().find_map(|(who, sel)| {
                sel.optional
                    .iter()
                    .find(|(dep, _)| !tried.contains(&(who.clone(), dep.clone())))
                    .map(|(dep, req)| (who.clone(), sel.version.clone(), dep.clone(), req.clone()))
            });
            let Some((who, version, dep, req)) = next else {
                break;
            };
            tried.insert((who.clone(), dep.clone()));
            if assigned.get(&dep).is_some_and(|s| req.matches(&s.version)) {
                continue;
            }
            let mut with = roots.clone();
            with.entry(dep)
                .or_default()
                .push((format!("{who} {version} (optional)"), req));
            let mut ignored = None;
            if let Some(a) = self.solve_once(&with, &mut ignored).await? {
                roots = with;
                assigned = a;
            }
        }

        let mut plan = Plan::default();
        for (name, sel) in assigned {
            match sel.pick {
                Some(p) => {
                    plan.picks.insert(name, p);
                }
                None => {
                    plan.kept.insert(name, sel.version);
                }
            }
        }
        Ok(plan)
    }

    async fn solve_once(
        &self,
        roots: &Constraints,
        last: &mut Option<Error>,
    ) -> Result<Option<Assigned>> {
        let mut assigned = Assigned::new();
        if self.solve(roots, &mut assigned, last).await? {
            Ok(Some(assigned))
        } else {
            Ok(None)
        }
    }

    fn solve<'a>(
        &'a self,
        constraints: &'a Constraints,
        assigned: &'a mut Assigned,
        last: &'a mut Option<Error>,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            if let Some(cycle) = self.find_cycle(assigned) {
                let first = cycle.first().cloned().expect("a cycle has packages");
                let version = assigned.get(&first).map(|s| s.version.clone());
                *last = Some(Error::Conflict {
                    at: match &version {
                        Some(v) => At::new(&first).version(v),
                        None => At::new(&first),
                    },
                    message: format!(
                        "dependency cycle: {}",
                        cycle
                            .iter()
                            .map(Name::to_string)
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                });
                return Ok(false);
            }

            let Some(name) = constraints
                .keys()
                .find(|n| !assigned.contains_key(*n))
                .cloned()
            else {
                return Ok(true);
            };
            let reqs = &constraints[&name];
            let in_range = |v: &Version| reqs.iter().all(|(_, r)| r.matches(v));
            let mut tried_any = false;
            let mut first_skip: Option<Error> = None;

            // An installed version that fits is the first candidate.
            let mut kept_version = None;
            if !self.force.contains(&name)
                && let Some(inst) = self.installed.get(&name)
                && in_range(&inst.version)
            {
                tried_any = true;
                kept_version = Some(inst.version.clone());
                let sel = Selected {
                    version: inst.version.clone(),
                    deps: inst.dependencies.clone(),
                    optional: Vec::new(),
                    pick: None,
                };
                if self
                    .try_select(&name, sel, constraints, assigned, last)
                    .await?
                {
                    return Ok(true);
                }
            }

            if let Some((idx, offers)) = self.locate(&name).await? {
                for offer in offers.iter().filter(|o| in_range(&o.version)) {
                    if kept_version.as_ref() == Some(&offer.version) {
                        continue;
                    }
                    match self.evaluate(idx, offer).await {
                        Err(skip) => {
                            first_skip.get_or_insert(skip);
                        }
                        Ok(pick) => {
                            tried_any = true;
                            let (deps, optional) = self.dependencies_of(&pick);
                            let sel = Selected {
                                version: offer.version.clone(),
                                deps,
                                optional,
                                pick: Some(pick),
                            };
                            if self
                                .try_select(&name, sel, constraints, assigned, last)
                                .await?
                            {
                                return Ok(true);
                            }
                        }
                    }
                }
            }

            if !tried_any {
                *last = Some(match first_skip {
                    Some(e) => e,
                    None if reqs.len() > 1 => Error::Conflict {
                        at: At::new(&name),
                        message: format!(
                            "no version satisfies {}",
                            reqs.iter()
                                .map(|(who, r)| format!("{who} requires {r}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    },
                    None => Error::NotFound {
                        at: At::new(&name).version(range_text(reqs)),
                    },
                });
            }
            Ok(false)
        })
    }

    /// Tries one candidate: checks it against installed dependents and
    /// assigned packages, adds its dependencies and solves the rest.
    async fn try_select(
        &self,
        name: &Name,
        sel: Selected,
        constraints: &Constraints,
        assigned: &mut Assigned,
        last: &mut Option<Error>,
    ) -> Result<bool> {
        let ver = sel.version.clone();
        if let Some((dependent, range)) = self.broken_dependent(name, &ver, assigned) {
            *last = Some(conflict(
                name,
                &ver,
                format!("breaks installed {dependent}, which requires {name} {range}"),
            ));
            return Ok(false);
        }
        let mut next = constraints.clone();
        for (dep, req) in &sel.deps {
            if let Some(existing) = assigned.get(dep)
                && !req.matches(&existing.version)
            {
                *last = Some(conflict(
                    name,
                    &ver,
                    format!(
                        "requires {dep} {req}, which conflicts with {dep} {}",
                        existing.version
                    ),
                ));
                return Ok(false);
            }
            next.entry(dep.clone())
                .or_default()
                .push((format!("{name} {ver}"), req.clone()));
        }
        assigned.insert(name.clone(), sel);
        if self.solve(&next, assigned, last).await? {
            return Ok(true);
        }
        assigned.remove(name);
        Ok(false)
    }

    /// The dependencies and optional dependencies of a pick, from the
    /// targets that are installed.
    fn dependencies_of(&self, pick: &Pick) -> (Deps, Deps) {
        let mut deps = Vec::new();
        let mut optional = Vec::new();
        for t in &pick.targets {
            if let Some(info) = pick.details.target(t) {
                deps.extend(info.dependencies.iter().cloned());
                optional.extend(info.optional_dependencies.iter().cloned());
            }
        }
        (deps, optional)
    }

    /// An installed package that stays as it is and needs a different
    /// version of `name` than `version`.
    fn broken_dependent(
        &self,
        name: &Name,
        version: &Version,
        assigned: &Assigned,
    ) -> Option<(Name, VersionReq)> {
        for (other, inst) in &self.installed {
            if other == name || assigned.contains_key(other) || self.force.contains(other) {
                continue;
            }
            for (dep, req) in &inst.dependencies {
                if dep == name && !req.matches(version) {
                    return Some((other.clone(), req.clone()));
                }
            }
        }
        None
    }

    fn find_cycle(&self, assigned: &Assigned) -> Option<Vec<Name>> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Mark {
            Visiting,
            Done,
        }

        fn visit<'a>(
            node: &'a Name,
            r: &'a Resolver,
            assigned: &'a Assigned,
            marks: &mut HashMap<&'a Name, Mark>,
            stack: &mut Vec<&'a Name>,
        ) -> Option<Vec<Name>> {
            if let Some(m) = marks.get(node) {
                if *m == Mark::Visiting {
                    let pos = stack.iter().position(|n| *n == node).unwrap_or(0);
                    let mut cycle: Vec<Name> = stack[pos..].iter().map(|n| (*n).clone()).collect();
                    cycle.push(node.clone());
                    return Some(cycle);
                }
                return None;
            }
            marks.insert(node, Mark::Visiting);
            stack.push(node);
            let deps: Vec<&Name> = match (assigned.get(node), r.installed.get(node)) {
                (Some(s), _) => s.deps.iter().map(|(n, _)| n).collect(),
                (None, Some(i)) => i.dependencies.iter().map(|(n, _)| n).collect(),
                (None, None) => Vec::new(),
            };
            for d in deps {
                if let Some(c) = visit(d, r, assigned, marks, stack) {
                    return Some(c);
                }
            }
            stack.pop();
            marks.insert(node, Mark::Done);
            None
        }

        let mut marks = HashMap::new();
        let mut stack = Vec::new();
        assigned
            .keys()
            .find_map(|n| visit(n, self, assigned, &mut marks, &mut stack))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Requirement;
    use crate::platform::Platform;
    use crate::source::{VariantInfo, Withdrawn};
    use crate::target::StaticTarget;

    const DAY: u64 = 86_400;

    struct Fake {
        label: String,
        packages: BTreeMap<Name, Vec<Offer>>,
        calls: Mutex<Vec<String>>,
        lazy: bool,
    }

    impl Fake {
        fn new(label: &str, offers: Vec<Offer>) -> Fake {
            let mut packages: BTreeMap<Name, Vec<Offer>> = BTreeMap::new();
            for o in offers {
                packages.entry(o.name.clone()).or_default().push(o);
            }
            Fake {
                label: label.into(),
                packages,
                calls: Mutex::new(Vec::new()),
                lazy: false,
            }
        }
    }

    impl Source for Fake {
        fn label(&self) -> &str {
            &self.label
        }

        fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>> {
            Box::pin(async move { Ok(self.packages.keys().cloned().collect()) })
        }

        fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>> {
            Box::pin(async move {
                let mut list = self.packages.get(name).cloned().unwrap_or_default();
                if self.lazy {
                    for o in &mut list {
                        o.details = None;
                    }
                }
                Ok(list)
            })
        }

        fn details<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Details>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(offer.version.to_string());
                let found = self.packages[&offer.name]
                    .iter()
                    .find(|o| o.version == offer.version)
                    .unwrap();
                Ok(found.details.clone().unwrap())
            })
        }

        fn fetch<'a>(&'a self, _: &'a Offer) -> BoxFuture<'a, Result<crate::Egg>> {
            Box::pin(async { Err(Error::other("not used")) })
        }
    }

    fn info(name: &str) -> TargetInfo {
        TargetInfo {
            name: name.into(),
            build: false,
            requires: Vec::new(),
            dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            variants: Vec::new(),
        }
    }

    fn req(s: &str) -> VersionReq {
        VersionReq::parse(s).unwrap()
    }

    trait InfoExt {
        fn dep(self, name: &str, range: &str) -> Self;
        fn opt(self, name: &str, range: &str) -> Self;
        fn requires(self, fact: &str, values: &[&str]) -> Self;
        fn variant(self, os: Option<&str>, arch: Option<&str>) -> Self;
    }

    impl InfoExt for TargetInfo {
        fn dep(mut self, name: &str, range: &str) -> Self {
            self.dependencies
                .push((Name::parse(name).unwrap(), req(range)));
            self
        }
        fn opt(mut self, name: &str, range: &str) -> Self {
            self.optional_dependencies
                .push((Name::parse(name).unwrap(), req(range)));
            self
        }
        fn requires(mut self, fact: &str, values: &[&str]) -> Self {
            self.requires.push(Requirement {
                fact: fact.into(),
                values: values.iter().map(|s| s.to_string()).collect(),
            });
            self
        }
        fn variant(mut self, os: Option<&str>, arch: Option<&str>) -> Self {
            self.variants.push(VariantInfo {
                os: os.map(Into::into),
                arch: arch.map(Into::into),
                sha256: None,
                signature: None,
                published: None,
            });
            self
        }
    }

    fn offer(name: &str, version: &str, targets: Vec<TargetInfo>) -> Offer {
        let mut o = Offer::new(
            Name::parse(name).unwrap(),
            Version::parse(version).unwrap(),
            "fake",
        );
        o.details = Some(Details {
            about: Default::default(),
            targets,
        });
        o
    }

    fn crow() -> Arc<dyn Target> {
        Arc::new(StaticTarget::new("crow").fact("crow", "1.2.0"))
    }

    fn resolver_with(offers: Vec<Offer>, min_age: u64) -> Resolver {
        let fake = Arc::new(Fake::new("fake", offers));
        Resolver::new(
            vec![SourceEntry::registry(
                fake,
                vec![],
                Duration::from_secs(min_age),
            )],
            vec![crow()],
            Timestamp::parse("2026-11-17T00:00:00Z").unwrap(),
            Seen::default(),
        )
    }

    fn roots(list: &[(&str, &str)]) -> Constraints {
        list.iter()
            .map(|(n, r)| {
                (
                    Name::parse(n).unwrap(),
                    vec![("request".to_string(), req(r))],
                )
            })
            .collect()
    }

    fn solve(r: &Resolver, list: &[(&str, &str)]) -> Result<Plan> {
        crate::block_on(r.resolve(roots(list)))
    }

    fn versions(plan: &Plan) -> BTreeMap<String, String> {
        plan.picks
            .iter()
            .map(|(n, p)| (n.to_string(), p.offer.version.to_string()))
            .chain(
                plan.kept
                    .iter()
                    .map(|(n, v)| (n.to_string(), format!("{v} (kept)"))),
            )
            .collect()
    }

    #[test]
    fn picks_the_newest_in_range_with_dependencies() {
        let r = resolver_with(
            vec![
                offer("g/a", "1.0.0", vec![info("crow").dep("g/b", "^1")]),
                offer("g/a", "1.1.0", vec![info("crow").dep("g/b", "^1.1")]),
                offer("g/a", "2.0.0", vec![info("crow")]),
                offer("g/b", "1.0.0", vec![info("crow")]),
                offer("g/b", "1.1.5", vec![info("crow")]),
                offer("g/b", "2.0.0", vec![info("crow")]),
            ],
            0,
        );
        let plan = solve(&r, &[("g/a", "^1")]).unwrap();
        let v = versions(&plan);
        assert_eq!(v["g/a"], "1.1.0");
        assert_eq!(v["g/b"], "1.1.5");
        assert_eq!(plan.picks[&Name::parse("g/a").unwrap()].targets, ["crow"]);
    }

    #[test]
    fn only_installed_targets_contribute_dependencies() {
        let r = resolver_with(
            vec![
                offer(
                    "g/a",
                    "1.0.0",
                    vec![
                        info("crow").dep("g/b", "*"),
                        info("rookery").dep("g/c", "*"),
                    ],
                ),
                offer("g/b", "1.0.0", vec![info("crow")]),
                offer("g/c", "1.0.0", vec![info("rookery")]),
            ],
            0,
        );
        let plan = solve(&r, &[("g/a", "*")]).unwrap();
        assert_eq!(
            plan.picks.keys().map(|n| n.to_string()).collect::<Vec<_>>(),
            ["g/a", "g/b"]
        );
    }

    #[test]
    fn a_dependency_without_a_group_is_in_the_packages_group() {
        // Details carry full names, so the group default is applied when they are made.
        let manifest = crate::Manifest::parse(
            "format 1\npackage {\n group aiviarium\n id a\n version \"1.0.0\"\n}\ntargets {\n crow {\n dependencies {\n b \"^1\"\n }\n }\n}\n",
        )
        .unwrap();
        let details = Details::from_manifest(&manifest);
        assert_eq!(
            details.targets[0].dependencies[0].0,
            Name::parse("aiviarium/b").unwrap()
        );
    }

    #[test]
    fn a_package_without_the_clients_target_is_skipped() {
        let r = resolver_with(
            vec![
                offer("g/a", "1.0.0", vec![info("crow")]),
                offer("g/a", "1.1.0", vec![info("rookery")]),
            ],
            0,
        );
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );
        let r = resolver_with(vec![offer("g/a", "1.1.0", vec![info("rookery")])], 0);
        let err = solve(&r, &[("g/a", "*")]).unwrap_err();
        assert!(err.to_string().contains("no target for crow"), "{err}");
    }

    #[test]
    fn optional_dependencies_are_kept_when_they_fit_and_dropped_when_not() {
        let r = resolver_with(
            vec![
                offer(
                    "g/a",
                    "1.0.0",
                    vec![
                        info("crow")
                            .opt("g/fits", "^1")
                            .opt("g/clash", "^2")
                            .opt("g/gone", "*"),
                    ],
                ),
                offer("g/fits", "1.0.0", vec![info("crow").dep("g/shared", "^1")]),
                offer("g/clash", "2.0.0", vec![info("crow").dep("g/shared", "^2")]),
                offer("g/shared", "1.0.0", vec![info("crow")]),
                offer("g/shared", "2.0.0", vec![info("crow")]),
            ],
            0,
        );
        let v = versions(&solve(&r, &[("g/a", "*")]).unwrap());
        assert_eq!(v["g/fits"], "1.0.0");
        assert_eq!(v["g/shared"], "1.0.0");
        assert!(!v.contains_key("g/clash"));
        assert!(!v.contains_key("g/gone"));
    }

    #[test]
    fn skips_to_an_older_version_when_requirements_are_unmet() {
        let r = resolver_with(
            vec![
                offer("g/a", "1.0.0", vec![info("crow").requires("crow", &["^1"])]),
                offer("g/a", "1.1.0", vec![info("crow").requires("crow", &["^2"])]),
            ],
            0,
        );
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );

        let r = resolver_with(
            vec![offer(
                "g/a",
                "1.1.0",
                vec![info("crow").requires("crow", &["^2"])],
            )],
            0,
        );
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::Unmet {
                target,
                fact,
                wanted,
                found,
                at,
            } => {
                assert_eq!(target, "crow");
                assert_eq!(fact, "crow");
                assert_eq!(wanted, ["^2"]);
                assert_eq!(found.as_deref(), Some("1.2.0"));
                assert_eq!(at.version.as_deref(), Some("1.1.0"));
            }
            e => panic!("{e}"),
        }
    }

    #[test]
    fn requirements_of_every_installed_target_hold() {
        let two = |c: &str| vec![info("crow"), info("rookery").requires("rookery", &[c])];
        let fake = Arc::new(Fake::new("fake", vec![offer("g/a", "1.0.0", two(">=0.7"))]));
        let targets: Vec<Arc<dyn Target>> = vec![
            crow(),
            Arc::new(StaticTarget::new("rookery").fact("rookery", "0.6.0")),
        ];
        let r = Resolver::new(
            vec![SourceEntry::registry(fake, vec![], Duration::ZERO)],
            targets,
            Timestamp::from_secs(0),
            Seen::default(),
        );
        assert!(
            matches!(solve(&r, &[("g/a", "*")]), Err(Error::Unmet { target, .. }) if target == "rookery")
        );
    }

    #[test]
    fn selects_variants_and_reports_no_variant() {
        let linux_arm = Arc::new(Fake::new(
            "fake",
            vec![
                offer(
                    "g/a",
                    "1.0.0",
                    vec![
                        info("crow")
                            .variant(None, Some("aarch64"))
                            .variant(None, Some("x86_64")),
                    ],
                ),
                offer(
                    "g/a",
                    "1.1.0",
                    vec![info("crow").variant(Some("windows"), None)],
                ),
            ],
        ));
        let mk = |platform: Platform, f: Arc<Fake>| {
            Resolver::new(
                vec![SourceEntry::registry(f, vec![], Duration::ZERO)],
                vec![Arc::new(
                    StaticTarget::new("crow").variants(Variants::Matching(platform)),
                )],
                Timestamp::from_secs(0),
                Seen::default(),
            )
        };
        let r = mk(Platform::new("linux", "aarch64"), linux_arm.clone());
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );

        let r = mk(Platform::new("linux", "riscv64"), linux_arm.clone());
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::NoVariant {
                target,
                platform,
                at,
            } => {
                assert_eq!(target, "crow");
                assert_eq!(platform, Platform::new("linux", "riscv64"));
                assert_eq!(at.version.as_deref(), Some("1.1.0"));
            }
            e => panic!("{e}"),
        }

        // A fallback variant matches anything; a target on demand needs none.
        let fb = Arc::new(Fake::new(
            "fake",
            vec![offer(
                "g/a",
                "1.0.0",
                vec![info("crow").variant(None, None)],
            )],
        ));
        assert!(solve(&mk(Platform::new("linux", "riscv64"), fb), &[("g/a", "*")]).is_ok());
        let on_demand = Resolver::new(
            vec![SourceEntry::registry(linux_arm, vec![], Duration::ZERO)],
            vec![Arc::new(
                StaticTarget::new("crow").variants(Variants::OnDemand),
            )],
            Timestamp::from_secs(0),
            Seen::default(),
        );
        assert_eq!(
            versions(&solve(&on_demand, &[("g/a", "*")]).unwrap())["g/a"],
            "1.1.0"
        );
    }

    #[test]
    fn not_built_names_the_targets_that_need_a_build() {
        let mut built = info("crow");
        built.build = true;
        let mut o = offer("g/a", "1.0.0", vec![built]);
        o.built = false;
        let mut plain = offer("g/b", "1.0.0", vec![info("crow")]);
        plain.built = false;
        let r = resolver_with(vec![o, plain], 0);
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::NotBuilt { targets, .. } => assert_eq!(targets, ["crow"]),
            e => panic!("{e}"),
        }
        // Source without a build command installs from the tag.
        assert!(solve(&r, &[("g/b", "*")]).is_ok());
    }

    #[test]
    fn withdrawn_versions_are_skipped() {
        let mut bad = offer("g/a", "1.1.0", vec![info("crow")]);
        bad.withdrawn = Some(Withdrawn {
            reason: Some("broke sign-in".into()),
        });
        let r = resolver_with(
            vec![offer("g/a", "1.0.0", vec![info("crow")]), bad.clone()],
            0,
        );
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );

        let r = resolver_with(vec![bad], 0);
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::Withdrawn { reason, .. } => assert_eq!(reason.as_deref(), Some("broke sign-in")),
            e => panic!("{e}"),
        }
    }

    #[test]
    fn not_found_names_the_range() {
        let r = resolver_with(vec![offer("g/a", "1.0.0", vec![info("crow")])], 0);
        match solve(&r, &[("g/a", "^2")]).unwrap_err() {
            Error::NotFound { at } => {
                assert_eq!(at.package, "g/a");
                assert_eq!(at.version.as_deref(), Some("^2"));
            }
            e => panic!("{e}"),
        }
        assert!(matches!(
            solve(&r, &[("g/nope", "*")]),
            Err(Error::NotFound { .. })
        ));
    }

    fn published(mut o: Offer, at: &str) -> Offer {
        o.published = Some(Timestamp::parse(at).unwrap());
        o
    }

    #[test]
    fn too_young_versions_fall_back_and_fail_with_the_time() {
        let r = resolver_with(
            vec![
                published(
                    offer("g/a", "1.0.0", vec![info("crow")]),
                    "2026-11-01T00:00:00Z",
                ),
                published(
                    offer("g/a", "1.1.0", vec![info("crow")]),
                    "2026-11-16T00:00:00Z",
                ),
            ],
            3 * DAY,
        );
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );

        let r = resolver_with(
            vec![published(
                offer("g/a", "1.1.0", vec![info("crow")]),
                "2026-11-16T00:00:00Z",
            )],
            3 * DAY,
        );
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::TooYoung { until, at } => {
                assert_eq!(until, Timestamp::parse("2026-11-19T00:00:00Z").unwrap());
                assert_eq!(at.version.as_deref(), Some("1.1.0"));
            }
            e => panic!("{e}"),
        }
    }

    #[test]
    fn the_installed_version_is_never_held() {
        // An update looks the installed package up again; a young newer
        // version must leave it where it is rather than fail.
        let r = resolver_with(
            vec![
                published(
                    offer("g/a", "1.0.0", vec![info("crow")]),
                    "2026-11-16T00:00:00Z",
                ),
                published(
                    offer("g/a", "1.1.0", vec![info("crow")]),
                    "2026-11-16T12:00:00Z",
                ),
            ],
            3 * DAY,
        );
        let mut r = with_installed(r, &[("g/a", "1.0.0", &[])]);
        r.force(Name::parse("g/a").unwrap());
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );
    }

    #[test]
    fn allow_young_applies_to_the_named_package_only() {
        let offers = vec![
            published(
                offer("g/a", "1.0.0", vec![info("crow").dep("g/b", "*")]),
                "2026-11-16T00:00:00Z",
            ),
            published(
                offer("g/b", "1.0.0", vec![info("crow")]),
                "2026-11-16T00:00:00Z",
            ),
        ];
        let mut r = resolver_with(offers.clone(), 3 * DAY);
        r.allow_young(Name::parse("g/a").unwrap());
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::TooYoung { at, .. } => assert_eq!(at.package, "g/b"),
            e => panic!("{e}"),
        }
        let mut r = resolver_with(offers, 3 * DAY);
        r.allow_young(Name::parse("g/a").unwrap());
        r.allow_young(Name::parse("g/b").unwrap());
        assert!(solve(&r, &[("g/a", "*")]).is_ok());
    }

    #[test]
    fn a_version_without_publish_time_counts_from_when_it_was_first_seen() {
        let name = Name::parse("g/a").unwrap();
        let old = Timestamp::parse("2026-11-10T00:00:00Z").unwrap();
        let mut seen = Seen::default();
        seen.record("fake", &name, [&Version::new(1, 0, 0)], old);
        let fake = Arc::new(Fake::new(
            "fake",
            vec![
                offer("g/a", "1.0.0", vec![info("crow")]),
                offer("g/a", "1.1.0", vec![info("crow")]),
            ],
        ));
        let r = Resolver::new(
            vec![SourceEntry::registry(
                fake,
                vec![],
                Duration::from_secs(3 * DAY),
            )],
            vec![crow()],
            Timestamp::parse("2026-11-17T00:00:00Z").unwrap(),
            seen,
        );
        // 1.0.0 was first seen a week ago; 1.1.0 only now.
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );
    }

    #[test]
    fn the_backdating_guard_uses_first_seen_for_late_versions() {
        let name = Name::parse("g/a").unwrap();
        let long_ago = "2026-01-01T00:00:00Z";
        let day = |d: &str| Timestamp::parse(&format!("2026-11-{d}T00:00:00Z")).unwrap();
        let mk = |offers: Vec<Offer>, now: Timestamp, seen: Seen| {
            let fake = Arc::new(Fake::new("fake", offers));
            Resolver::new(
                vec![SourceEntry::registry(
                    fake,
                    vec![],
                    Duration::from_secs(3 * DAY),
                )],
                vec![crow()],
                now,
                seen,
            )
        };
        let v1 = published(offer("g/a", "1.0.0", vec![info("crow")]), long_ago);
        let v2 = published(offer("g/a", "1.1.0", vec![info("crow")]), long_ago);

        // The first listing counts from the publish time: no wait.
        let r = mk(vec![v1.clone()], day("10"), Seen::default());
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );
        let seen = r.state().seen.clone();
        assert!(
            seen.get("fake", &name, &Version::new(1, 0, 0))
                .unwrap()
                .first_listing
        );

        // A later listing with a new version that claims an old time is held.
        let r = mk(vec![v1.clone(), v2.clone()], day("12"), seen);
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.0.0"
        );
        assert_eq!(
            r.held_until(0, &v2),
            Some(day("15")),
            "counted from the day it was first seen"
        );
        let seen = r.state().seen.clone();

        // Three days after it was first seen, it installs.
        let r = mk(vec![v1, v2], day("15"), seen);
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.1.0"
        );
    }

    #[test]
    fn tolerates_diamonds_by_backtracking() {
        let r = resolver_with(
            vec![
                offer(
                    "g/a",
                    "1.0.0",
                    vec![info("crow").dep("g/b", "^1").dep("g/c", "*")],
                ),
                offer("g/b", "1.0.0", vec![info("crow")]),
                offer("g/b", "2.0.0", vec![info("crow")]),
                offer("g/c", "1.0.0", vec![info("crow").dep("g/b", "^1")]),
                offer("g/c", "2.0.0", vec![info("crow").dep("g/b", "^2")]),
            ],
            0,
        );
        let v = versions(&solve(&r, &[("g/a", "*")]).unwrap());
        assert_eq!((v["g/b"].as_str(), v["g/c"].as_str()), ("1.0.0", "1.0.0"));
    }

    #[test]
    fn reports_a_conflict_when_no_set_holds() {
        let r = resolver_with(
            vec![
                offer(
                    "g/a",
                    "1.0.0",
                    vec![info("crow").dep("g/b", "^1").dep("g/c", "*")],
                ),
                offer("g/b", "1.0.0", vec![info("crow")]),
                offer("g/c", "1.0.0", vec![info("crow").dep("g/b", "^2")]),
                offer("g/b", "2.0.0", vec![info("crow")]),
            ],
            0,
        );
        assert!(matches!(
            solve(&r, &[("g/a", "*")]),
            Err(Error::Conflict { .. })
        ));
    }

    #[test]
    fn cycles_are_conflicts() {
        let r = resolver_with(
            vec![
                offer("g/a", "1.0.0", vec![info("crow").dep("g/b", "*")]),
                offer("g/b", "1.0.0", vec![info("crow").dep("g/a", "*")]),
            ],
            0,
        );
        match solve(&r, &[("g/a", "*")]).unwrap_err() {
            Error::Conflict { message, .. } => assert!(message.contains("cycle"), "{message}"),
            e => panic!("{e}"),
        }
    }

    type Row<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    fn with_installed(mut r: Resolver, list: &[Row]) -> Resolver {
        let installed = list
            .iter()
            .map(|(n, v, deps)| {
                (
                    Name::parse(n).unwrap(),
                    InstalledPkg {
                        version: Version::parse(v).unwrap(),
                        dependencies: deps
                            .iter()
                            .map(|(d, rg)| (Name::parse(d).unwrap(), req(rg)))
                            .collect(),
                    },
                )
            })
            .collect();
        r.set_installed(installed);
        r
    }

    #[test]
    fn installed_packages_that_fit_are_kept() {
        let r = resolver_with(
            vec![
                offer("g/a", "1.0.0", vec![info("crow").dep("g/b", "^1")]),
                offer("g/b", "1.0.0", vec![info("crow")]),
                offer("g/b", "1.5.0", vec![info("crow")]),
            ],
            0,
        );
        let mut r = with_installed(r, &[("g/b", "1.0.0", &[])]);
        r.force(Name::parse("g/a").unwrap());
        let v = versions(&solve(&r, &[("g/a", "*")]).unwrap());
        assert_eq!(v["g/a"], "1.0.0");
        assert_eq!(v["g/b"], "1.0.0 (kept)");
    }

    #[test]
    fn installed_dependents_hold_a_package_back() {
        let offers = vec![
            offer("g/b", "1.0.0", vec![info("crow")]),
            offer("g/b", "2.0.0", vec![info("crow")]),
        ];
        let mut r = with_installed(
            resolver_with(offers.clone(), 0),
            &[("g/a", "1.0.0", &[("g/b", "^1")]), ("g/b", "1.0.0", &[])],
        );
        r.force(Name::parse("g/b").unwrap());
        match solve(&r, &[("g/b", "^2")]).unwrap_err() {
            Error::Conflict { message, .. } => assert!(message.contains("g/a"), "{message}"),
            e => panic!("{e}"),
        }

        // An update of g/b is constrained to the dependent's range.
        let mut r = with_installed(
            resolver_with(offers, 0),
            &[("g/a", "1.0.0", &[("g/b", "^1")]), ("g/b", "1.0.0", &[])],
        );
        r.force(Name::parse("g/b").unwrap());
        let mut c = Constraints::new();
        c.insert(
            Name::parse("g/b").unwrap(),
            vec![("g/a".to_string(), req("^1"))],
        );
        let plan = crate::block_on(r.resolve(c)).unwrap();
        assert_eq!(versions(&plan)["g/b"], "1.0.0");
    }

    #[test]
    fn details_are_fetched_only_for_versions_considered() {
        let mut fake = Fake::new(
            "fake",
            vec![
                offer("g/a", "1.0.0", vec![info("crow")]),
                offer("g/a", "1.1.0", vec![info("crow")]),
                offer("g/a", "1.2.0", vec![info("crow")]),
            ],
        );
        fake.lazy = true;
        let fake = Arc::new(fake);
        let r = Resolver::new(
            vec![SourceEntry::registry(fake.clone(), vec![], Duration::ZERO)],
            vec![crow()],
            Timestamp::from_secs(0),
            Seen::default(),
        );
        assert_eq!(
            versions(&solve(&r, &[("g/a", "*")]).unwrap())["g/a"],
            "1.2.0"
        );
        assert_eq!(*fake.calls.lock().unwrap(), ["1.2.0"]);
    }

    #[test]
    fn finds_a_name_by_id_alone() {
        let r = resolver_with(
            vec![
                offer("one/a", "1.0.0", vec![info("crow")]),
                offer("one/b", "1.0.0", vec![info("crow")]),
                offer("two/b", "1.0.0", vec![info("crow")]),
            ],
            0,
        );
        assert_eq!(
            crate::block_on(r.find_name("a")).unwrap(),
            Name::parse("one/a").unwrap()
        );
        assert!(matches!(
            crate::block_on(r.find_name("b")),
            Err(Error::Ambiguous { groups, .. }) if groups == ["one", "two"]
        ));
        assert!(matches!(
            crate::block_on(r.find_name("c")),
            Err(Error::NotFound { .. })
        ));
    }

    #[test]
    fn registries_are_searched_in_order_and_only_limits_groups() {
        let first = Arc::new(Fake::new(
            "first",
            vec![offer("mine/a", "1.0.0", vec![info("crow")])],
        ));
        let second = Arc::new(Fake::new(
            "second",
            vec![
                offer("mine/a", "9.0.0", vec![info("crow")]),
                offer("other/a", "2.0.0", vec![info("crow")]),
            ],
        ));
        let r = Resolver::new(
            vec![
                SourceEntry::registry(first, vec!["mine".into()], Duration::ZERO),
                SourceEntry::registry(second, vec!["other".into()], Duration::ZERO),
            ],
            vec![crow()],
            Timestamp::from_secs(0),
            Seen::default(),
        );
        let v = versions(&solve(&r, &[("mine/a", "*"), ("other/a", "*")]).unwrap());
        assert_eq!(v["mine/a"], "1.0.0");
        assert_eq!(v["other/a"], "2.0.0");
    }

    #[test]
    fn newest_ignores_requirements_but_not_withdrawn_or_age() {
        let unmet = published(
            offer("g/a", "1.2.0", vec![info("crow").requires("gpu", &[])]),
            "2026-11-01T00:00:00Z",
        );
        let mut withdrawn = offer("g/a", "1.3.0", vec![info("crow")]);
        withdrawn.withdrawn = Some(Withdrawn { reason: None });
        let young = published(
            offer("g/a", "1.4.0", vec![info("crow")]),
            "2026-11-16T00:00:00Z",
        );
        let r = resolver_with(vec![unmet, withdrawn, young], 3 * DAY);
        let (_, o) = crate::block_on(r.newest(&Name::parse("g/a").unwrap(), &req("*"))).unwrap();
        assert_eq!(o.version, Version::new(1, 2, 0));
    }
}

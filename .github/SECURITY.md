# Security

## What Hatchery trusts

Hatchery installs files that other people wrote. It never runs a package's
code on the machine that installs it. These checks tell you whether the files are the ones you meant.

- **Checksums.** An egg's SHA-256 comes from the registry's index or from the
  lock file. Bytes that differ are refused.
- **Signatures.** A registry with a `key` accepts only eggs signed by it. The
  signature covers the package, version, file name, checksum and publish time.
- **Git registries.** A package installed from a tag has no signature. What
  protects it is git's own transport and the commit in `eggs.lock`. If a tag
  later points at another commit, Hatchery refuses to install it.
- **Unpacking.** An egg with `..`, an absolute path, a symlink or a hardlink is
  refused.
- **Minimum release age.** A new version waits before it is installed. The default is 3 days for a registry read over the network. One that shows up later with an old publish time counts from when it was first seen.

Hatchery doesn't check what a target does with its files. That is for the tool that installs the target.

## Keys and tokens

Signing keys and registry tokens are read from environment variables only.
Don't put them in `hatchery.kdl`, in a command line or in a repository.
`hatchery keygen` prints a new key to store as a CI secret.

## Supported versions

Fixes go into the latest release. Hatchery is at 0.x and older releases aren't
maintained.

## Reporting a vulnerability

Please don't open a public issue. Use **Report a vulnerability** under the
repository's Security tab, which opens a private report to the maintainers.

Say what you found, how to reproduce it and what an attacker gains. You'll get
an answer within a week. Once a fix is released, the report is published with
credit to you, unless you'd rather not be named.

An unsigned or changed egg that passes a registry with a key is a vulnerability. So is a write outside the install folder, or code that runs during an install. A registry without a key serving what it likes is not.

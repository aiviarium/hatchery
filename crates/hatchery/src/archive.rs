//! A deterministic POSIX ustar writer and a strict reader for gzipped tar
//! archives. The reader refuses `..`, absolute paths, symlinks, hardlinks
//! and special files, and names that are unsafe to unpack on some systems:
//! `.git`, names Windows cannot hold and paths that differ only by case.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use bytes::Bytes;
use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};

use crate::egg::Entry;
use crate::error::{Error, IoContext, Result};

/// The most one entry may hold: 512 MiB.
pub const MAX_ENTRY_BYTES: u64 = 512 << 20;
/// The most an archive may hold unpacked: 512 MiB.
pub const MAX_TOTAL_BYTES: u64 = 512 << 20;
/// What the tar stream (headers, padding, skipped data) may add to the
/// bytes of the files.
const STREAM_ALLOWANCE: u64 = 64 << 20;
/// The most entries an archive may have.
const MAX_ENTRIES: usize = 100_000;
/// The most a PAX or GNU long-name header may hold.
const MAX_META_BYTES: u64 = 16 << 20;
/// The largest size an octal size field holds.
const MAX_OCTAL_SIZE: u64 = 0o77_777_777_777;

fn bad(message: impl std::fmt::Display) -> Error {
    Error::UnsafeArchive(message.to_string())
}

// Writing.

/// Writes entries as a gzipped ustar archive. Entries are sorted by path,
/// all files, with mode 0644 or 0755 and no owner or time, so the same
/// entries always give the same bytes.
pub fn write(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    for e in &sorted {
        validate_entry_path(&e.path, false)?;
        if !seen.insert(e.path.as_str()) {
            return Err(bad(format!("duplicate entry {:?}", e.path)));
        }
    }
    if let Some(problem) = collision(sorted.iter().map(|e| e.path.as_str())) {
        return Err(bad(problem));
    }
    let gz = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::default());
    let mut tar = TarWriter { inner: gz };
    for e in sorted {
        tar.add_file(e).at_path("archive")?;
    }
    tar.finish().at_path("archive")
}

struct TarWriter<W: Write> {
    inner: W,
}

impl<W: Write> TarWriter<W> {
    fn add_file(&mut self, e: &Entry) -> std::io::Result<()> {
        let size = e.data.len() as u64;
        let mode = if e.executable { 0o755 } else { 0o644 };
        let (name, prefix, needs_pax) = split_ustar_path(&e.path);
        if needs_pax || size > MAX_OCTAL_SIZE {
            let mut pax = String::new();
            if needs_pax {
                pax.push_str(&pax_record("path", &e.path));
            }
            if size > MAX_OCTAL_SIZE {
                pax.push_str(&pax_record("size", &size.to_string()));
            }
            let pax_name = format!(
                "PaxHeaders/{}",
                e.path.rsplit('/').next().unwrap_or("entry")
            );
            let (pax_path, _, _) = split_ustar_path(&pax_name);
            let hdr = build_header(&pax_path, "", 0o644, pax.len() as u64, b'x');
            self.inner.write_all(&hdr)?;
            self.inner.write_all(pax.as_bytes())?;
            write_padding(&mut self.inner, pax.len() as u64)?;
        }
        let hdr_size = if size <= MAX_OCTAL_SIZE { size } else { 0 };
        self.inner
            .write_all(&build_header(&name, &prefix, mode, hdr_size, b'0'))?;
        self.inner.write_all(&e.data)?;
        write_padding(&mut self.inner, size)
    }
}

impl TarWriter<flate2::write::GzEncoder<Vec<u8>>> {
    fn finish(mut self) -> std::io::Result<Vec<u8>> {
        self.inner.write_all(&[0u8; 1024])?;
        self.inner.finish()
    }
}

/// Splits a path into the ustar name and prefix fields; the flag says the
/// path does not fit and needs a PAX header.
fn split_ustar_path(path: &str) -> (String, String, bool) {
    if path.len() <= 100 {
        return (path.to_string(), String::new(), false);
    }
    for (idx, ch) in path.char_indices() {
        if ch == '/' {
            let prefix = &path[..idx];
            let name = &path[idx + 1..];
            if !prefix.is_empty() && prefix.len() <= 155 && !name.is_empty() && name.len() <= 100 {
                return (name.to_string(), prefix.to_string(), false);
            }
        }
    }
    // The PAX record carries the real path; this is only a placeholder.
    let mut cut = 100;
    while !path.is_char_boundary(cut) {
        cut -= 1;
    }
    (path[..cut].to_string(), String::new(), true)
}

fn pax_record(key: &str, val: &str) -> String {
    let body = format!(" {key}={val}\n");
    let mut len = body.len() + 1;
    loop {
        let s = format!("{len}{body}");
        if s.len() == len {
            return s;
        }
        len = s.len();
    }
}

fn build_header(name: &str, prefix: &str, mode: u32, size: u64, typeflag: u8) -> [u8; 512] {
    let mut hdr = [0u8; 512];
    copy_bytes(&mut hdr[0..100], name.as_bytes());
    copy_bytes(&mut hdr[100..108], format!("{mode:07o}\0").as_bytes());
    copy_bytes(&mut hdr[108..116], b"0000000\0");
    copy_bytes(&mut hdr[116..124], b"0000000\0");
    copy_bytes(&mut hdr[124..136], format!("{size:011o}\0").as_bytes());
    copy_bytes(&mut hdr[136..148], b"00000000000\0");
    hdr[148..156].fill(b' ');
    hdr[156] = typeflag;
    copy_bytes(&mut hdr[257..263], b"ustar\0");
    copy_bytes(&mut hdr[263..265], b"00");
    if !prefix.is_empty() {
        copy_bytes(&mut hdr[345..500], prefix.as_bytes());
    }
    let sum: u32 = hdr.iter().map(|&b| u32::from(b)).sum();
    copy_bytes(&mut hdr[148..156], format!("{sum:06o}\0 ").as_bytes());
    hdr
}

fn copy_bytes(dst: &mut [u8], src: &[u8]) {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
}

fn write_padding<W: Write>(w: &mut W, size: u64) -> std::io::Result<()> {
    let rem = (size % 512) as usize;
    if rem == 0 {
        return Ok(());
    }
    w.write_all(&[0u8; 512][..512 - rem])
}

// Reading.

/// Reads a gzipped tar archive into entries, refusing anything unsafe to
/// unpack. Directory entries are dropped; the result is sorted by path.
pub fn read(bytes: &[u8]) -> Result<Vec<Entry>> {
    read_limited(bytes, MAX_TOTAL_BYTES)
}

/// Like [`read`], with a smaller limit on the bytes the files may hold
/// together. The decompressed stream is also capped, and so is the number
/// of entries, so a bomb ends quickly.
pub fn read_limited(bytes: &[u8], max_total: u64) -> Result<Vec<Entry>> {
    let mut reader = GzDecoder::new(bytes).take(max_total.saturating_add(STREAM_ALLOWANCE));
    let mut count = 0usize;
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = 0u64;
    scan_tar(&mut reader, |t| {
        count += 1;
        if count > MAX_ENTRIES {
            return Err(bad(format!("archive has more than {MAX_ENTRIES} entries")));
        }
        let Some(rel) = validate_entry_path(&t.path, t.is_dir)? else {
            return Ok(());
        };
        if t.is_dir {
            return Ok(());
        }
        total += t.data.len() as u64;
        if total > max_total {
            return Err(bad(format!(
                "archive holds more than {max_total} bytes unpacked"
            )));
        }
        if !seen.insert(rel.clone()) {
            return Err(bad(format!("duplicate entry {:?}", t.path)));
        }
        entries.push(Entry {
            path: rel,
            data: Bytes::from(t.data),
            executable: t.mode & 0o111 != 0,
        });
        Ok(())
    })?;
    if let Some(problem) = collision(entries.iter().map(|e| e.path.as_str())) {
        return Err(bad(problem));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

struct TarEntry {
    path: String,
    mode: u32,
    is_dir: bool,
    data: Vec<u8>,
}

fn io_err(e: std::io::Error) -> Error {
    bad(format!("cannot read archive: {e}"))
}

fn scan_tar<R: Read>(
    reader: &mut R,
    mut on_entry: impl FnMut(TarEntry) -> Result<()>,
) -> Result<()> {
    let mut hdr = [0u8; 512];
    let mut pax_path: Option<String> = None;
    let mut pax_size: Option<u64> = None;
    let mut gnu_longname: Option<String> = None;

    loop {
        if !read_exact_or_eof(reader, &mut hdr)? {
            break;
        }
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        verify_checksum(&hdr)?;

        let typeflag = hdr[156];
        let raw_size = parse_octal(&hdr[124..136])?;
        let mode = parse_octal(&hdr[100..108]).unwrap_or(0o644) as u32;
        let size = pax_size.take().unwrap_or(raw_size);
        if size > MAX_ENTRY_BYTES {
            return Err(bad(format!(
                "tar entry size {size} exceeds maximum {MAX_ENTRY_BYTES}"
            )));
        }
        let header_name = ustar_path(&hdr)?;
        let path = gnu_longname
            .take()
            .or_else(|| pax_path.take())
            .unwrap_or(header_name);

        match typeflag {
            b'x' | b'L' | b'g' => {
                if size > MAX_META_BYTES {
                    return Err(bad("extended header is too large"));
                }
                let mut data = read_data(reader, size)?;
                skip_padding(reader, size)?;
                match typeflag {
                    b'x' => {
                        let parsed = parse_pax(&data)?;
                        pax_path = parsed.0;
                        pax_size = parsed.1;
                    }
                    b'L' => {
                        while data.last() == Some(&0) {
                            data.pop();
                        }
                        gnu_longname = Some(
                            String::from_utf8(data).map_err(|_| bad("non-UTF-8 GNU long name"))?,
                        );
                    }
                    // A global header is read and ignored.
                    _ => {}
                }
                continue;
            }
            b'2' => return Err(bad(format!("symlink entry {path:?} is not allowed"))),
            b'1' => return Err(bad(format!("hardlink entry {path:?} is not allowed"))),
            b'0' | b'\0' | b'5' => {}
            other => {
                return Err(bad(format!(
                    "unsupported tar entry type {:?} for {path:?}",
                    char::from(other)
                )));
            }
        }

        let is_dir = typeflag == b'5';
        let data = if is_dir {
            // A directory carries no data, whatever its size says.
            skip_exact(reader, size)?;
            Vec::new()
        } else {
            read_data(reader, size)?
        };
        skip_padding(reader, size)?;
        on_entry(TarEntry {
            path,
            mode,
            is_dir,
            data,
        })?;
    }
    Ok(())
}

fn read_data<R: Read>(r: &mut R, size: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let got = r.take(size).read_to_end(&mut data).map_err(io_err)?;
    if got as u64 != size {
        return Err(bad("unexpected end of archive in an entry"));
    }
    Ok(data)
}

fn skip_exact<R: Read>(r: &mut R, size: u64) -> Result<()> {
    let got = std::io::copy(&mut r.take(size), &mut std::io::sink()).map_err(io_err)?;
    if got != size {
        return Err(bad("unexpected end of archive in an entry"));
    }
    Ok(())
}

fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<bool> {
    let mut off = 0;
    while off < buf.len() {
        let n = r.read(&mut buf[off..]).map_err(io_err)?;
        if n == 0 {
            if off == 0 {
                return Ok(false);
            }
            return Err(bad("truncated tar header"));
        }
        off += n;
    }
    Ok(true)
}

fn skip_padding<R: Read>(r: &mut R, size: u64) -> Result<()> {
    let rem = (size % 512) as usize;
    if rem == 0 {
        return Ok(());
    }
    let mut buf = [0u8; 512];
    r.read_exact(&mut buf[..512 - rem])
        .map_err(|_| bad("unexpected end of archive in entry padding"))
}

fn verify_checksum(hdr: &[u8; 512]) -> Result<()> {
    let expected = parse_octal(&hdr[148..156])?;
    let actual: u64 = hdr
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(b)
            }
        })
        .sum();
    if actual != expected {
        return Err(bad(format!(
            "invalid tar header checksum (expected {expected:#o}, got {actual:#o})"
        )));
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let s = std::str::from_utf8(field)
        .map_err(|_| bad("non-UTF-8 tar number"))?
        .trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).map_err(|_| bad(format!("invalid tar number {s:?}")))
}

fn ustar_path(hdr: &[u8; 512]) -> Result<String> {
    let field = |r: std::ops::Range<usize>| -> Result<&str> {
        let b = &hdr[r];
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        std::str::from_utf8(&b[..end]).map_err(|_| bad("non-UTF-8 tar entry name"))
    };
    let name = field(0..100)?;
    let prefix = field(345..500)?;
    Ok(if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    })
}

fn parse_pax(data: &[u8]) -> Result<(Option<String>, Option<u64>)> {
    let (mut path, mut size) = (None, None);
    let mut pos = 0;
    while pos < data.len() {
        let rest = &data[pos..];
        let Some(sp) = rest.iter().position(|&b| b == b' ') else {
            return Err(bad("malformed PAX record"));
        };
        let rec_len: usize = std::str::from_utf8(&rest[..sp])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("invalid PAX record length"))?;
        if rec_len <= sp + 1 || pos + rec_len > data.len() {
            return Err(bad("PAX record length out of bounds"));
        }
        let rec = &data[pos + sp + 1..pos + rec_len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Some(eq) = rec.iter().position(|&b| b == b'=') {
            let key = std::str::from_utf8(&rec[..eq]).map_err(|_| bad("non-UTF-8 PAX key"))?;
            let val =
                std::str::from_utf8(&rec[eq + 1..]).map_err(|_| bad("non-UTF-8 PAX value"))?;
            match key {
                "path" => path = Some(val.to_string()),
                "size" => {
                    size = Some(val.parse().map_err(|_| bad("invalid PAX size"))?);
                }
                _ => {}
            }
        }
        pos += rec_len;
    }
    Ok((path, size))
}

/// Checks a tar entry path so it can never escape the folder it is
/// unpacked into. Returns the cleaned `/`-separated path, or None for the
/// root directory entry `.`.
fn validate_entry_path(raw: &str, is_dir: bool) -> Result<Option<String>> {
    if raw.is_empty() {
        return Err(bad("entry with an empty path"));
    }
    if raw.contains('\0') {
        return Err(bad("entry with a NUL byte in its path"));
    }
    if raw.contains('\\') {
        return Err(bad(format!("path {raw:?} has a backslash")));
    }
    if raw.starts_with('/') {
        return Err(bad(format!("path {raw:?} is absolute")));
    }
    // Drive letters are refused whatever the host system is.
    if raw.as_bytes().get(1) == Some(&b':') && raw.as_bytes()[0].is_ascii_alphabetic() {
        return Err(bad(format!("path {raw:?} is absolute")));
    }
    let trimmed = raw.strip_prefix("./").unwrap_or(raw);
    let trimmed = if is_dir {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
    if is_dir && (trimmed.is_empty() || trimmed == ".") {
        return Ok(None);
    }
    if !is_dir && raw.ends_with('/') {
        return Err(bad(format!("file path {raw:?} ends with a slash")));
    }
    for seg in trimmed.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(bad(format!("path {raw:?} escapes or is invalid")));
        }
        if let Some(why) = component_problem(seg) {
            return Err(bad(format!("path {raw:?} {why}")));
        }
    }
    Ok(Some(trimmed.to_string()))
}

/// Windows device names, which cannot be files whatever the extension.
const DEVICE_NAMES: [&str; 4] = ["con", "prn", "aux", "nul"];

/// Why one component of a path is unsafe to unpack, or None. `.git` would
/// let an egg run code when git runs in the install folder, and the other
/// names are refused because Windows reads them as something else or
/// cannot hold them.
pub(crate) fn component_problem(seg: &str) -> Option<String> {
    let lower = seg.to_ascii_lowercase();
    if lower == ".git" || lower == "git~1" {
        return Some("has a .git component".to_string());
    }
    if let Some(c) = seg
        .chars()
        .find(|c| c.is_control() || "<>:\"|?*".contains(*c))
    {
        return Some(format!("has {c:?}, which Windows does not allow in names"));
    }
    if seg.ends_with(['.', ' ']) {
        // Also covers `.git.` and `.git `, which Windows reads as `.git`.
        return Some("has a component ending in a dot or a space".to_string());
    }
    let stem = lower.split('.').next().unwrap_or("");
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
    };
    if DEVICE_NAMES.contains(&stem) || numbered("com") || numbered("lpt") {
        return Some("has a component that is a Windows device name".to_string());
    }
    None
}

/// Finds paths that would overwrite each other on a system that ignores
/// case, or where a file would also be a folder.
fn collision<'a>(paths: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
    let mut files: HashMap<String, &str> = HashMap::new();
    for p in paths.clone() {
        if let Some(other) = files.insert(p.to_lowercase(), p)
            && other != p
        {
            return Some(format!("{other:?} and {p:?} differ only by case"));
        }
    }
    for p in paths {
        let mut at = p;
        while let Some((parent, _)) = at.rsplit_once('/') {
            if let Some(file) = files.get(&parent.to_lowercase()) {
                return Some(format!("{file:?} is both a file and a folder"));
            }
            at = parent;
        }
    }
    None
}

/// What is wrong with the paths of files to pack, in words for an error,
/// or None when all of them are safe to unpack.
pub(crate) fn unsafe_paths<'a>(paths: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
    for p in paths.clone() {
        if let Some(why) = p.split('/').find_map(component_problem) {
            return Some(format!("{p:?} {why}"));
        }
    }
    collision(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, data: &str, executable: bool) -> Entry {
        Entry {
            path: path.to_string(),
            data: Bytes::from(data.to_string()),
            executable,
        }
    }

    fn make_raw_targz(entries: Vec<(&str, u8, &[u8])>) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        for (path, typeflag, data) in entries {
            let mut hdr = [0u8; 512];
            copy_bytes(&mut hdr[0..100], path.as_bytes());
            copy_bytes(&mut hdr[100..108], b"0000644\0");
            copy_bytes(&mut hdr[108..116], b"0000000\0");
            copy_bytes(&mut hdr[116..124], b"0000000\0");
            let size = if typeflag == b'0' {
                data.len() as u64
            } else {
                0
            };
            copy_bytes(&mut hdr[124..136], format!("{size:011o}\0").as_bytes());
            copy_bytes(&mut hdr[136..148], b"00000000000\0");
            hdr[148..156].fill(b' ');
            hdr[156] = typeflag;
            if typeflag == b'2' || typeflag == b'1' {
                copy_bytes(&mut hdr[157..257], b"../target");
            }
            copy_bytes(&mut hdr[257..263], b"ustar\0");
            copy_bytes(&mut hdr[263..265], b"00");
            let sum: u32 = hdr.iter().map(|&b| u32::from(b)).sum();
            copy_bytes(&mut hdr[148..156], format!("{sum:06o}\0 ").as_bytes());
            gz.write_all(&hdr).unwrap();
            if typeflag == b'0' && !data.is_empty() {
                gz.write_all(data).unwrap();
                write_padding(&mut gz, size).unwrap();
            }
        }
        gz.write_all(&[0u8; 1024]).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn round_trips_and_is_deterministic() {
        let a = vec![
            entry("b/run.sh", "#!/bin/sh\n", true),
            entry("a.txt", "hello", false),
            entry("empty", "", false),
        ];
        let mut b = a.clone();
        b.reverse();
        let one = write(&a).unwrap();
        assert_eq!(one, write(&b).unwrap());
        // Fixed gzip header: no name, no time, unknown system.
        assert_eq!(&one[..10], &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255][..]);
        let back = read(&one).unwrap();
        let paths: Vec<_> = back.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b/run.sh", "empty"]);
        assert!(back[1].executable);
        assert!(!back[0].executable);
        assert_eq!(back[0].data.as_ref(), b"hello");
    }

    #[test]
    fn refuses_unsafe_entries() {
        let manifest = b"{}";
        let cases = [
            ("parent dir", "../escape.txt", b'0'),
            ("nested parent dir", "a/../../escape.txt", b'0'),
            ("dot-slash then parent", "./../escape.txt", b'0'),
            ("absolute path", "/tmp/evil.txt", b'0'),
            ("backslash traversal", r"..\evil.txt", b'0'),
            ("windows drive path", "C:/evil.txt", b'0'),
            ("symlink", "link", b'2'),
            ("hardlink", "hard", b'1'),
            ("fifo", "pipe", b'6'),
        ];
        for (label, bad_path, kind) in cases {
            let pkg = make_raw_targz(vec![("ok.json", b'0', manifest), (bad_path, kind, b"pwn")]);
            let err = read(&pkg).unwrap_err();
            assert!(matches!(err, Error::UnsafeArchive(_)), "{label}: {err:?}");
        }
    }

    #[test]
    fn refuses_duplicates_bad_checksums_and_garbage() {
        let dup = make_raw_targz(vec![("a", b'0', b"1"), ("a", b'0', b"2")]);
        assert!(matches!(read(&dup), Err(Error::UnsafeArchive(_))));

        let clash = make_raw_targz(vec![("a", b'0', b"1"), ("a/b", b'0', b"2")]);
        assert!(matches!(read(&clash), Err(Error::UnsafeArchive(_))));

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        let mut hdr = build_header("a", "", 0o644, 0, b'0');
        hdr[0] = b'b';
        gz.write_all(&hdr).unwrap();
        gz.write_all(&[0u8; 1024]).unwrap();
        let broken = gz.finish().unwrap();
        assert!(matches!(read(&broken), Err(Error::UnsafeArchive(_))));

        assert!(matches!(read(b"not gzip"), Err(Error::UnsafeArchive(_))));
    }

    #[test]
    fn refuses_git_windows_and_case_colliding_names() {
        let refused = [
            ".git/config",
            "a/.GIT/config",
            ".Git",
            "GIT~1/config",
            "git~1/config",
            ".git./config",
            ".git /config",
            ".git::$INDEX_ALLOCATION/config",
            "a<b",
            "a>b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b",
            "a\x01b",
            "con",
            "CON.txt",
            "d/nul.txt",
            "Aux",
            "prn.tar.gz",
            "com1",
            "COM9.txt",
            "lpt3",
            "trailing.",
            "trailing ",
        ];
        for path in refused {
            let pkg = make_raw_targz(vec![(path, b'0', b"x")]);
            let err = read(&pkg).unwrap_err();
            assert!(matches!(err, Error::UnsafeArchive(_)), "{path:?}: {err:?}");
            assert!(write(&[entry(path, "x", false)]).is_err(), "{path:?}");
        }
        for path in [
            "gitignore",
            ".github/a",
            "a.git",
            "com0",
            "com10",
            "lpt",
            "console",
            "x/.gitignore",
        ] {
            let pkg = make_raw_targz(vec![(path, b'0', b"x")]);
            assert!(read(&pkg).is_ok(), "{path:?}");
        }
    }

    #[test]
    fn refuses_paths_that_differ_only_by_case() {
        let cases = [
            vec![("a.txt", b'0', &b"1"[..]), ("A.TXT", b'0', b"2")],
            vec![("A", b'0', b"1"), ("a/b", b'0', b"2")],
            vec![("d/x", b'0', b"1"), ("D/X", b'0', b"2")],
        ];
        for case in cases {
            let err = read(&make_raw_targz(case)).unwrap_err();
            assert!(matches!(err, Error::UnsafeArchive(_)), "{err:?}");
        }
        assert!(write(&[entry("a", "", false), entry("A", "", false)]).is_err());
        assert!(write(&[entry("A", "", false), entry("a/b", "", false)]).is_err());
    }

    #[test]
    fn accepts_directories_and_dot_root() {
        let pkg = make_raw_targz(vec![
            ("./", b'5', b""),
            ("d/", b'5', b""),
            ("d/f", b'0', b"x"),
        ]);
        let entries = read(&pkg).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "d/f");
    }

    #[test]
    fn write_refuses_unsafe_paths() {
        assert!(write(&[entry("../x", "", false)]).is_err());
        assert!(write(&[entry("/x", "", false)]).is_err());
        assert!(write(&[entry("a", "", false), entry("a", "", false)]).is_err());
    }

    #[test]
    fn supports_long_paths_via_ustar_prefix_and_pax() {
        let prefixed = format!("rules/{}/{}", "a".repeat(80), "f.txt");
        let pax = format!(
            "rules/{}/{}/very_long_file_name_to_trigger_pax_header_encoding.txt",
            "a".repeat(80),
            "b".repeat(80)
        );
        let non_ascii = format!("{}/é.txt", "é".repeat(60));
        let entries = vec![
            entry(&prefixed, "1", false),
            entry(&pax, "2", true),
            entry(&non_ascii, "3", false),
        ];
        let back = read(&write(&entries).unwrap()).unwrap();
        assert_eq!(back.len(), 3);
        for e in &entries {
            let got = back.iter().find(|b| b.path == e.path).unwrap();
            assert_eq!(got.data, e.data);
            assert_eq!(got.executable, e.executable);
        }
    }

    #[test]
    fn bombs_end_with_an_error() {
        // Many empty entries.
        let many: Vec<(String, u8, &[u8])> = (0..MAX_ENTRIES + 1)
            .map(|i| (format!("f{i}"), b'0', &b""[..]))
            .collect();
        let raw = make_raw_targz(many.iter().map(|(p, t, d)| (p.as_str(), *t, *d)).collect());
        assert!(raw.len() < 2 << 20);
        assert!(matches!(read(&raw), Err(Error::UnsafeArchive(_))));

        // A file of zeros larger than the limit given.
        let big = vec![0u8; 3 << 20];
        let raw = make_raw_targz(vec![("a", b'0', &big)]);
        assert!(raw.len() < 64 << 10);
        assert!(read_limited(&raw, 1 << 20).is_err());
        assert!(read_limited(&raw, 4 << 20).is_ok());
    }
}

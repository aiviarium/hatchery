//! A deterministic POSIX ustar writer and a strict reader for gzipped tar
//! archives. The reader refuses `..`, absolute paths, symlinks, hardlinks
//! and special files.

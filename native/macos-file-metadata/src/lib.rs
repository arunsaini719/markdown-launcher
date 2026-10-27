use std::{fs::File, io};

/// Preserve macOS ownership, permissions, ACLs, and extended attributes without
/// copying source data or exposing raw copyfile-state pointers to callers.
pub fn copy_metadata(source: &File, destination: &File) -> io::Result<()> {
    use rustix::fs::{CopyfileFlags, copyfile_state_alloc, copyfile_state_free, fcopyfile};
    let state = copyfile_state_alloc()?;
    // SAFETY: state was allocated above, is exclusively owned here, and remains
    // valid until the single matching free below. Both file handles stay alive.
    let copied = unsafe { fcopyfile(source, destination, state, CopyfileFlags::METADATA) };
    // SAFETY: no operation uses state after this free.
    let freed = unsafe { copyfile_state_free(state) };
    copied.and(freed).map_err(Into::into)
}

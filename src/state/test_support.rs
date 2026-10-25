//! Controlled metadata edits for valid test fixtures; corruption cases use raw records.
use super::{State, profiles::Index, write_json};
pub(super) fn edit_index<T>(state: &State, change: impl FnOnce(&mut Index) -> T) -> T {
    let _metadata = state.metadata_lock().unwrap();
    let mut index = state.read_index().unwrap();
    let result = change(&mut index);
    write_json(&state.root.join("index.json"), &index).unwrap();
    result
}

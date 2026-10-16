//! `CoreSettings`: the plain, validated settings app-core runs with.
//!
//! No persistence here: the persisted copy (`config.json`, atomic writes, schema 1) stays in
//! `src-tauri/config.rs` because it is coupled to Tauri and logging. The adapter converts it into
//! `CoreSettings` and calls `apply_settings` after `update_settings` has validated and saved a patch.
//! Planned sections: `driver { auto_stream }` (default off, owner decision Q7), relay endpoint, recording
//! directory, engine configuration (fuel, alert, tyre, brake).
//!
//! Stub: implemented with the facade in increment I-4.

//! Every error and warning code the app can show, in one table. Codes are
//! frozen: the README "Error codes" section links to each by `#code`, and a
//! test (tests/codes.rs) fails if the source emits a code missing here.

pub const CODES: &[&str] = &[
    // Profile load (errors)
    "profile-syntax",
    "profile-bound",
    "profile-overlap",
    "profile-schema-too-new",
    "profile-too-large",
    "profile-not-utf8",
    // Profile load (warnings)
    "profile-unknown-field",
    "profile-unknown-group",
    "profile-unknown-icon",
    "profile-dock",
    "profile-theme",
    // Profile files
    "profile-save-failed",
    "profile-backup-failed",
    "profile-changed-on-disk",
    "pad-create-failed",
    // Buttons
    "key-unknown",
    "action-unknown",
    "system-key-blocked",
    "unknown-button",
    // Sending keys
    "target-is-pad",
    "target-untitled",
    "target-not-allowed",
    "target-elevated",
    "target-elevation-unknown",
    "aborted-focus-change",
    "send-incomplete",
    "dropped-queue-full",
    "sink-unavailable",
    "sink-permission",
    "worker-stopped",
    // App
    "internal-error",
    "log-unwritable",
    "settings-unreadable",
    // Editing
    "edit-active",
    "edit-stale",
    "edit-max-buttons",
    "layout-grid-full",
    "layout-out-of-grid",
    "layout-button-outside-grid",
    "restore-failed",
    "editor-open-failed",
    "region-unavailable",
    // Go-to-pad buttons
    "pad-not-found",
];

pub fn is_known(code: &str) -> bool {
    CODES.contains(&code)
}

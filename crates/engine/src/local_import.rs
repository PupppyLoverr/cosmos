//! One-time local→synced profile import.
//!
//! A device that worked locally (PR #30's `profiles/local`) and then signs in
//! gets its chats, spaces, journals, and command-ledger claims carried into
//! the synced profile — through the same write paths every live mutation
//! uses, so nothing here invents a second persistence or sync mechanism:
//!
//!  - chat docs land via `save_snapshot_with_cursor(chat_id, bytes, 0, CHAT2_DOC_EPOCH)`,
//!    which is exactly the "born chat2" shape: cursor 0 makes the first room
//!    join push the doc's full update log from VV zero (doc_host first-contact
//!    push), and epoch 2 keeps `DocHost::open` off the s2 discard-and-adopt
//!    branch that would drop the transcript.
//!  - registry rows go through `WorkspaceHost::import_chat_row` /
//!    `import_space_row` (live upserts: persisted, pushed, watched).
//!  - attachments are NOT copied or rewritten. Transcripts embed absolute
//!    paths under the local profile's uploads root, so that root becomes a
//!    read-only jail root of the synced profile — the exact mechanism the
//!    legacy-uploads adoption already uses (`EngineProfile::claim_legacy_uploads_root`).
//!    The marker file re-arms the root on every later synced boot.
//!
//! Idempotence is structural: a chat/space whose row already exists in the
//! target is skipped, so re-running after a partial import (or after new
//! local work from a later signed-out stretch) imports only what's missing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeron_doc::{REGISTRY_DOC_ID, RegistryDoc};
use zeron_sync::DocsStore;

use crate::EngineError;
use crate::chat2_host::CHAT2_DOC_EPOCH;
use crate::run_journal::journal_paths;
use crate::uploads::Uploads;
use crate::workspace_host::WorkspaceHost;

/// Marker recording completed imports, at `{data_dir}/local-import.json`.
/// One entry per target (org, user): the same device may sign into several
/// accounts, and each gets its own one-time import.
const MARKER_FILE: &str = "local-import.json";

/// Serializes every marker read-modify-write in this process. Two runtimes in
/// one process (a swap mid-flight, tests) must not lose an account's entry to
/// a racing update; cross-process exclusion is the data-dir `InstanceLock`'s job.
fn marker_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Marker {
    #[serde(default)]
    imports: Vec<MarkerEntry>,
    #[serde(default)]
    read_roots: Vec<ReadRootGrant>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkerEntry {
    org_id: String,
    user_id: String,
    imported_at_ms: i64,
    imported_chats: usize,
    imported_spaces: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadRootGrant {
    org_id: String,
    user_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalChatSummary {
    pub chat_id: String,
    pub title: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
}

/// What the wizard needs to offer (or silently skip) the import step.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImportStatus {
    /// Chats in the local profile with no row in this synced workspace yet.
    pub available_chats: usize,
    /// Spaces in the local profile with no row in this synced workspace yet.
    pub available_spaces: usize,
    /// A completed import for this (org, user) is already on record.
    pub imported_before: bool,
}

/// Per-item progress for the wizard's progress step.
/// NB: `rename_all` renames the *variants* (the `kind` tag); struct-variant
/// *fields* need `rename_all_fields` — without it the wire shape silently
/// ships snake_case counters (caught by `import_events_serialize_camel_case`).
#[derive(Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ImportEvent {
    /// Emitted once, before any copying.
    Start { chats: usize, spaces: usize },
    /// One imported chat (docs + journal + row).
    Chat {
        index: usize,
        total: usize,
        chat_id: String,
        title: Option<String>,
    },
    /// Terminal summary — also the RPC's final stream item.
    Summary {
        imported_chats: usize,
        imported_spaces: usize,
        skipped_chats: usize,
        skipped_spaces: usize,
        journals_copied: usize,
        ledger_rows_merged: usize,
        errors: Vec<String>,
    },
}

/// The import service a synced runtime carries (`EngineCore::assemble` builds
/// one when the profile is account-scoped and a local profile exists on disk).
#[derive(Clone)]
pub struct LocalImporter {
    inner: Arc<ImporterInner>,
}

struct ImporterInner {
    data_dir: PathBuf,
    device_id: String,
    org_id: String,
    user_id: String,
    target_store: Arc<DocsStore>,
    target_journals: PathBuf,
    workspace: WorkspaceHost,
    uploads: Uploads,
}

impl LocalImporter {
    #[allow(clippy::too_many_arguments)] // engine assembly seam, not a public API
    pub fn new(
        data_dir: &Path,
        device_id: &str,
        org_id: &str,
        user_id: &str,
        target_store: Arc<DocsStore>,
        target_journals: PathBuf,
        workspace: WorkspaceHost,
        uploads: Uploads,
    ) -> Self {
        Self {
            inner: Arc::new(ImporterInner {
                data_dir: data_dir.to_path_buf(),
                device_id: device_id.to_string(),
                org_id: org_id.to_string(),
                user_id: user_id.to_string(),
                target_store,
                target_journals,
                workspace,
                uploads,
            }),
        }
    }

    fn source_root(&self) -> PathBuf {
        self.inner.data_dir.join("profiles").join("local")
    }

    fn source_uploads(&self) -> PathBuf {
        self.source_root().join("uploads")
    }

    fn marker_path(&self) -> PathBuf {
        self.inner.data_dir.join(MARKER_FILE)
    }

    /// Load the marker under [`marker_lock`]. A file that exists but does not
    /// parse is moved aside to `local-import.json.corrupt` — its grants were
    /// already unreadable (boot's [`marker_grants_read_root`] parses the same
    /// file), and keeping the bytes as evidence beats silently clobbering them
    /// on the next write.
    fn load_marker_locked(&self) -> Marker {
        let path = self.marker_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => return Marker::default(),
        };
        match serde_json::from_str(&raw) {
            Ok(marker) => marker,
            Err(err) => {
                let aside = path.with_extension("json.corrupt");
                tracing::warn!(error = %err, aside = %aside.display(),
                    "local-import marker is corrupt; moving it aside");
                if let Err(err) = std::fs::rename(&path, &aside) {
                    tracing::warn!(error = %err, "corrupt marker aside failed");
                }
                Marker::default()
            }
        }
    }

    fn imported_before(&self) -> bool {
        let _guard = marker_lock();
        self.load_marker_locked()
            .imports
            .iter()
            .any(|e| e.org_id == self.inner.org_id && e.user_id == self.inner.user_id)
    }

    /// Record a completed import and re-arm the read-only uploads root for
    /// future boots (`EngineCore::assemble` consults [`marker_grants_read_root`]).
    ///
    /// The read-modify-write is serialized process-wide (cross-process is
    /// already excluded by the data-dir `InstanceLock`) and published
    /// atomically — write + fsync a sibling temp file, then rename over the
    /// marker — so a crash or short write can never truncate the file and
    /// erase other accounts' grants. Failures propagate to the caller and end
    /// up in the import summary.
    fn record_import(&self, chats: usize, spaces: usize) -> Result<(), EngineError> {
        let _guard = marker_lock();
        let mut marker = self.load_marker_locked();
        marker
            .imports
            .retain(|e| !(e.org_id == self.inner.org_id && e.user_id == self.inner.user_id));
        marker.imports.push(MarkerEntry {
            org_id: self.inner.org_id.clone(),
            user_id: self.inner.user_id.clone(),
            imported_at_ms: crate::now_ms(),
            imported_chats: chats,
            imported_spaces: spaces,
        });
        self.publish_marker_locked(&marker)
    }

    fn record_read_root_grant(&self) -> Result<(), EngineError> {
        let _guard = marker_lock();
        let mut marker = self.load_marker_locked();
        marker.read_roots.retain(|entry| {
            entry.org_id != self.inner.org_id || entry.user_id != self.inner.user_id
        });
        marker.read_roots.push(ReadRootGrant {
            org_id: self.inner.org_id.clone(),
            user_id: self.inner.user_id.clone(),
        });
        self.publish_marker_locked(&marker)
    }

    fn publish_marker_locked(&self, marker: &Marker) -> Result<(), EngineError> {
        let bytes = serde_json::to_vec_pretty(&marker)
            .map_err(|err| EngineError::Other(format!("marker serialize: {err}")))?;
        let path = self.marker_path();
        let tmp = path.with_extension("json.tmp");
        let publish = || -> std::io::Result<()> {
            {
                use std::io::Write;
                let mut file = std::fs::File::create(&tmp)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            std::fs::rename(&tmp, &path)
        };
        publish().map_err(|err| {
            let _ = std::fs::remove_file(&tmp);
            EngineError::Other(format!("marker write: {err}"))
        })
    }

    /// Open the local profile's stores read-only-ish. `None` when the device
    /// never ran a local profile (nothing to import).
    fn open_source(&self) -> Result<Option<(DocsStore, RegistryDoc)>, EngineError> {
        let root = self.source_root();
        if !root.join("docs.sqlite3").is_file() {
            return Ok(None);
        }
        let store = DocsStore::open(&root)?;
        let Some(bytes) = store.load_snapshot(REGISTRY_DOC_ID)? else {
            return Ok(None); // store exists but no registry replica — nothing rowed
        };
        let registry = RegistryDoc::from_bytes(&bytes, &self.inner.device_id)?;
        Ok(Some((store, registry)))
    }

    pub fn list_local_chats(&self) -> Result<Vec<LocalChatSummary>, EngineError> {
        let Some((_, registry)) = self.open_source()? else {
            return Ok(Vec::new());
        };
        let mut chats: Vec<_> = registry
            .read_chats()?
            .into_iter()
            .map(|chat| LocalChatSummary {
                chat_id: chat.id,
                title: chat.title,
                updated_at: chat.last_message_at.unwrap_or(chat.created_at),
                archived: chat.archived,
            })
            .collect();
        chats.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
        Ok(chats)
    }

    pub fn copy_local_chat_to_cloud(
        &self,
        local_chat_id: &str,
        new_chat_id: &str,
        device_id: &str,
        cloud_seat: zeron_proto::CloudSeatBinding,
    ) -> Result<(), EngineError> {
        let Some((source_store, registry)) = self.open_source()? else {
            return Err(EngineError::Other("local chat not found".into()));
        };
        let source_chat = registry
            .read_chats()?
            .into_iter()
            .find(|chat| chat.id == local_chat_id)
            .ok_or_else(|| EngineError::Other("local chat not found".into()))?;

        let source_uploads = self.source_uploads();
        if source_uploads.is_dir() {
            self.inner.uploads.add_read_only_root(&source_uploads);
            self.record_read_root_grant()?;
        }

        if let Some(bytes) = source_store.load_snapshot(local_chat_id)? {
            let bytes = rewrite_chat_snapshot(&bytes, local_chat_id, new_chat_id)?;
            self.inner.target_store.save_snapshot_with_cursor(
                new_chat_id,
                &bytes,
                0,
                CHAT2_DOC_EPOCH,
            )?;
        }

        let mut row = source_chat;
        row.id = new_chat_id.to_string();
        row.device_id = device_id.to_string();
        row.cwd = None;
        row.branch = None;
        row.checkout_id = None;
        row.source_context = None;
        row.harness_session_id = None;
        row.harness_session_cwd = None;
        row.space_id = None;
        row.last_seen_at = None;
        row.room_gen = Some(CHAT2_DOC_EPOCH);
        row.parent_chat_id = None;
        row.cloud_seat = Some(cloud_seat);
        row.cloud_run = None;
        row.cloud_backup = None;
        self.inner.workspace.import_chat_row(&row)?;
        Ok(())
    }

    /// What's importable right now (target-dedup applied).
    pub fn status(&self) -> Result<LocalImportStatus, EngineError> {
        let imported_before = self.imported_before();
        let Some((_, registry)) = self.open_source()? else {
            return Ok(LocalImportStatus {
                available_chats: 0,
                available_spaces: 0,
                imported_before,
            });
        };
        let mut available_chats = 0;
        for chat in registry.read_chats()? {
            if self.inner.workspace.chat(&chat.id)?.is_none() {
                available_chats += 1;
            }
        }
        let mut available_spaces = 0;
        for space in registry.read_spaces()? {
            if self.inner.workspace.space(&space.id)?.is_none() {
                available_spaces += 1;
            }
        }
        Ok(LocalImportStatus {
            available_chats,
            available_spaces,
            imported_before,
        })
    }

    /// Run the import, emitting [`ImportEvent`]s (the last is always
    /// `Summary`). Blocking (sqlite + fs) — callers run it off the async path.
    pub fn run(&self, mut emit: impl FnMut(ImportEvent)) -> Result<(), EngineError> {
        let Some((source_store, registry)) = self.open_source()? else {
            emit(ImportEvent::Start {
                chats: 0,
                spaces: 0,
            });
            emit(ImportEvent::Summary {
                imported_chats: 0,
                imported_spaces: 0,
                skipped_chats: 0,
                skipped_spaces: 0,
                journals_copied: 0,
                ledger_rows_merged: 0,
                errors: Vec::new(),
            });
            return Ok(());
        };

        let mut errors: Vec<String> = Vec::new();

        // Spaces first: chats reference `space_id`, and viewers resolve the
        // reference as soon as the chat row lands.
        let spaces = registry.read_spaces()?;
        let chats = registry.read_chats()?;
        let (total_chats, total_spaces) = (chats.len(), spaces.len());
        let pending_chats: Vec<_> = chats
            .into_iter()
            .filter(|chat| !matches!(self.inner.workspace.chat(&chat.id), Ok(Some(_))))
            .collect();
        let pending_spaces: Vec<_> = spaces
            .into_iter()
            .filter(|space| !matches!(self.inner.workspace.space(&space.id), Ok(Some(_))))
            .collect();
        let skipped_chats = total_chats - pending_chats.len();
        let skipped_spaces = total_spaces - pending_spaces.len();

        emit(ImportEvent::Start {
            chats: pending_chats.len(),
            spaces: pending_spaces.len(),
        });

        let mut imported_spaces = 0;
        for space in &pending_spaces {
            match self.inner.workspace.import_space_row(space) {
                Ok(()) => imported_spaces += 1,
                Err(err) => errors.push(format!("space {}: {err}", space.id)),
            }
        }

        let source_journals = self.source_root().join("journals");
        let total = pending_chats.len();
        let mut imported_chats = 0;
        let mut journals_copied = 0;
        for (index, chat) in pending_chats.iter().enumerate() {
            emit(ImportEvent::Chat {
                index,
                total,
                chat_id: chat.id.clone(),
                title: chat.title.clone(),
            });
            match self.import_chat(&source_store, chat, &source_journals) {
                Ok(journal) => {
                    imported_chats += 1;
                    if journal {
                        journals_copied += 1;
                    }
                }
                Err(err) => errors.push(format!("chat {}: {err}", chat.id)),
            }
        }

        // Merge the source's command ledger so imported pending commands can
        // never re-execute under this profile (mark-before-execute carries over).
        let ledger_rows_merged = source_store
            .processed_commands()
            .and_then(|rows| self.inner.target_store.import_processed_commands(&rows))
            .unwrap_or_else(|err| {
                errors.push(format!("command ledger: {err}"));
                0
            });

        // Transcripts embed absolute paths under the local uploads root; jail
        // it read-only now (and on future boots, via the marker). A marker
        // that fails to persist means imported attachments stop resolving
        // after a restart — that is a real failure, not a footnote.
        if self.source_uploads().is_dir() {
            self.inner
                .uploads
                .add_read_only_root(&self.source_uploads());
        }
        if let Err(err) = self.record_import(imported_chats, imported_spaces) {
            errors.push(format!("import marker: {err}"));
        }

        emit(ImportEvent::Summary {
            imported_chats,
            imported_spaces,
            skipped_chats,
            skipped_spaces,
            journals_copied,
            ledger_rows_merged,
            errors,
        });
        Ok(())
    }

    /// Copy one chat: doc snapshot (born-chat2 shape), journal files, row.
    /// Returns whether a journal file was copied.
    fn import_chat(
        &self,
        source_store: &DocsStore,
        chat: &zeron_proto::Chat,
        source_journals: &Path,
    ) -> Result<bool, EngineError> {
        // Doc bytes may be absent (a chat row created but never opened) — the
        // row alone is still worth carrying; a doc materializes on first open.
        if let Some(bytes) = source_store.load_snapshot(&chat.id)?
            && !self.inner.target_store.has_snapshot(&chat.id)?
        {
            self.inner.target_store.save_snapshot_with_cursor(
                &chat.id,
                &bytes,
                0,
                CHAT2_DOC_EPOCH,
            )?;
        }

        let mut copied = false;
        let (src_journal, src_resume) = journal_paths(source_journals, &chat.id);
        let (dst_journal, dst_resume) = journal_paths(&self.inner.target_journals, &chat.id);
        if src_journal.is_file() && !dst_journal.exists() {
            std::fs::create_dir_all(&self.inner.target_journals)?;
            std::fs::copy(&src_journal, &dst_journal)?;
            copied = true;
        }
        if src_resume.is_file() && !dst_resume.exists() {
            std::fs::create_dir_all(&self.inner.target_journals)?;
            std::fs::copy(&src_resume, &dst_resume)?;
        }

        // Row last: once it appears in watchers the chat is clickable, and by
        // then its doc + journal are already in place. Chat2 lineage is
        // explicit so `DocHost::open` never routes the import down the legacy
        // s2 adopt branch.
        let mut row = chat.clone();
        row.room_gen = Some(CHAT2_DOC_EPOCH);
        self.inner.workspace.import_chat_row(&row)?;
        Ok(copied)
    }
}

fn rewrite_chat_snapshot(
    bytes: &[u8],
    source_chat_id: &str,
    new_chat_id: &str,
) -> Result<Vec<u8>, EngineError> {
    let doc = loro::LoroDoc::new();
    doc.import(bytes)
        .map_err(|err| EngineError::Other(format!("local chat snapshot import: {err}")))?;
    let session = zeron_doc::SessionDoc::from_doc(doc);
    if let Some(embedded_chat_id) = session.chat_id() {
        if embedded_chat_id != source_chat_id {
            return Err(EngineError::Other(format!(
                "local chat document id mismatch: expected {source_chat_id}, got {embedded_chat_id}"
            )));
        }
        session.set_chat_id(new_chat_id)?;
    }
    Ok(session.export_snapshot()?)
}

/// Whether a recorded import grants the synced profile `(org, user)` the local
/// profile's uploads root as a read-only jail root. `EngineCore::assemble`
/// calls this on every account-scoped boot.
pub fn marker_grants_read_root(data_dir: &Path, org_id: &str, user_id: &str) -> Option<PathBuf> {
    let marker: Marker = std::fs::read_to_string(data_dir.join(MARKER_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())?;
    let hit = marker
        .imports
        .iter()
        .any(|e| e.org_id == org_id && e.user_id == user_id);
    let granted = marker
        .read_roots
        .iter()
        .any(|entry| entry.org_id == org_id && entry.user_id == user_id);
    let uploads = data_dir.join("profiles").join("local").join("uploads");
    ((hit || granted) && uploads.is_dir()).then_some(uploads)
}

#[cfg(test)]
mod tests {
    use super::{CHAT2_DOC_EPOCH, ImportEvent, LocalImporter, marker_grants_read_root};
    use chrono::Utc;
    use std::sync::Arc;
    use zeron_doc::{MessageRole, REGISTRY_DOC_ID, RegistryDoc, SessionDoc, SessionMessageEntry};
    use zeron_proto::{Chat, ChatConfig, CloudSeatBinding, CloudSeatKind, HarnessId, SandboxLevel};
    use zeron_sync::DocsStore;

    use crate::uploads::Uploads;
    use crate::workspace_host::{WorkspaceHost, WorkspaceHostConfig};

    #[test]
    fn import_events_serialize_camel_case() {
        let summary = serde_json::to_value(ImportEvent::Summary {
            imported_chats: 2,
            imported_spaces: 1,
            skipped_chats: 0,
            skipped_spaces: 0,
            journals_copied: 2,
            ledger_rows_merged: 3,
            errors: Vec::new(),
        })
        .expect("serialize");
        assert_eq!(summary["kind"], "summary");
        assert_eq!(
            summary["importedChats"], 2,
            "fields must be camelCase: {summary}"
        );
        let chat = serde_json::to_value(ImportEvent::Chat {
            index: 0,
            total: 2,
            chat_id: "c1".into(),
            title: None,
        })
        .expect("serialize");
        assert_eq!(chat["chatId"], "c1", "fields must be camelCase: {chat}");
    }

    #[tokio::test]
    async fn copy_local_chat_to_cloud_clones_docs_and_leaves_source_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        let source_root = data_dir.join("profiles").join("local");
        std::fs::create_dir_all(source_root.join("uploads")).unwrap();
        let source_store = DocsStore::open(&source_root).unwrap();
        let source_id = "local-chat";
        let now = Utc::now();
        let mut registry = RegistryDoc::new("local-device");
        let source_chat = Chat {
            id: source_id.into(),
            device_id: "local-device".into(),
            title: Some("Local title".into()),
            archived: true,
            cwd: Some("/local/workspace".into()),
            branch: Some("local-branch".into()),
            checkout_id: Some("local-checkout".into()),
            source_context: None,
            config: Some(ChatConfig {
                harness: HarnessId::Mock,
                model: Some("test-model".into()),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            last_message_preview: Some("hello".into()),
            last_message_at: Some(now),
            created_at: now,
            harness_session_id: Some("local-harness-session".into()),
            harness_session_cwd: Some("/local/workspace".into()),
            space_id: Some("local-space".into()),
            last_seen_at: Some(now),
            room_gen: Some(CHAT2_DOC_EPOCH),
            parent_chat_id: Some("local-parent".into()),
            cloud_seat: None,
            cloud_run: None,
            cloud_backup: None,
        };
        registry.upsert_chat(&source_chat).unwrap();
        source_store
            .save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes().unwrap())
            .unwrap();
        let source_registry_before = source_store
            .load_snapshot(REGISTRY_DOC_ID)
            .unwrap()
            .unwrap();

        let source_doc = SessionDoc::init(source_id).unwrap();
        source_doc
            .push_message(&SessionMessageEntry {
                id: "message-1".into(),
                role: MessageRole::User,
                parts: Vec::new(),
                created_at: now.timestamp_millis(),
                device_id: "local-device".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        source_store
            .save_snapshot(source_id, &source_doc.export_snapshot().unwrap())
            .unwrap();
        let source_bytes_before = source_store.load_snapshot(source_id).unwrap().unwrap();
        std::fs::write(
            source_root.join("uploads").join("attachment.png"),
            b"attachment",
        )
        .unwrap();

        let target_root = data_dir.join("orgs").join("org").join("user");
        let target_store = Arc::new(DocsStore::open(&target_root).unwrap());
        let workspace = WorkspaceHost::open(
            target_store.clone(),
            WorkspaceHostConfig {
                device_id: "synced-device".into(),
                device_name: "Synced device".into(),
                platform: "macos".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
            },
        )
        .unwrap();
        let uploads = Uploads::from_root(&target_root.join("uploads"));
        let importer = LocalImporter::new(
            data_dir,
            "synced-device",
            "org",
            "user",
            target_store.clone(),
            target_root.join("journals"),
            workspace.clone(),
            uploads,
        );
        let local_chats = importer.list_local_chats().unwrap();
        assert_eq!(local_chats.len(), 1);
        assert_eq!(local_chats[0].chat_id, source_id);
        assert_eq!(local_chats[0].title.as_deref(), Some("Local title"));
        assert!(local_chats[0].archived);
        let first_id = crate::new_id();
        let first_binding = CloudSeatBinding {
            seat_id: "seat".into(),
            kind: CloudSeatKind::Ssh,
            label: "Seat".into(),
            repo: None,
            workspace_path: Some(format!("/seat/{first_id}")),
            runner_device_id: None,
            branch: None,
            model_label: None,
            expires_at: None,
        };
        importer
            .copy_local_chat_to_cloud(source_id, &first_id, "synced-device", first_binding.clone())
            .unwrap();

        let copied = workspace.chat(&first_id).unwrap().unwrap();
        assert_ne!(copied.id, source_id);
        assert_eq!(copied.device_id, "synced-device");
        assert_eq!(copied.title, source_chat.title);
        assert_eq!(copied.config, source_chat.config);
        assert_eq!(copied.cloud_seat.as_ref(), Some(&first_binding));
        assert_eq!(copied.space_id, None);
        assert_eq!(copied.parent_chat_id, None);
        assert_eq!(copied.harness_session_id, None);
        assert_eq!(copied.harness_session_cwd, None);
        assert_eq!(copied.room_gen, Some(CHAT2_DOC_EPOCH));

        let copied_bytes = target_store.load_snapshot(&first_id).unwrap().unwrap();
        let copied_raw = loro::LoroDoc::new();
        copied_raw.import(&copied_bytes).unwrap();
        let copied_doc = SessionDoc::from_doc(copied_raw);
        assert_eq!(copied_doc.chat_id().as_deref(), Some(first_id.as_str()));
        assert_eq!(
            copied_doc.read_entries().unwrap(),
            source_doc.read_entries().unwrap()
        );
        assert_eq!(
            source_store.load_snapshot(source_id).unwrap().unwrap(),
            source_bytes_before
        );
        assert_eq!(
            source_store
                .load_snapshot(REGISTRY_DOC_ID)
                .unwrap()
                .unwrap(),
            source_registry_before
        );
        assert_eq!(
            marker_grants_read_root(data_dir, "org", "user"),
            Some(source_root.join("uploads"))
        );

        let second_id = crate::new_id();
        let mut second_binding = first_binding;
        second_binding.workspace_path = Some(format!("/seat/{second_id}"));
        importer
            .copy_local_chat_to_cloud(source_id, &second_id, "synced-device", second_binding)
            .unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(
            workspace.chat(&second_id).unwrap().unwrap().title,
            source_chat.title
        );
        assert_eq!(
            source_store.load_snapshot(source_id).unwrap().unwrap(),
            source_bytes_before
        );
    }
}

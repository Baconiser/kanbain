//! The agent library (D13): agent definitions, style blocks and skills as
//! files in the data folder, indexed in SQLite.
//!
//! ```text
//! <data>/agents/<name>.yaml
//! <data>/styles/<name>.md
//! <data>/skills/<name>/SKILL.md (+ supporting files)
//! ```
//!
//! The files are the source of truth: edits through the API write files,
//! and a watcher re-syncs the index when anything under these folders
//! changes, including edits made outside KanbAIn.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use db::models::agent_library::{AgentDefinitionRow, SkillRow, StyleBlockRow};
use executors::adapters::{
    AgentDefinition, ResolvedAgent, Skill, SkillFile, StyleBlock, default_agents,
    definition::{skill_frontmatter, validate_name},
};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use thiserror::Error;
use tokio::sync::Mutex;
use ts_rs::TS;

/// Largest skill (all files together).
pub const MAX_SKILL_BYTES: usize = 10 * 1024 * 1024;
/// Largest import (zip or uploaded files together).
pub const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum AgentLibraryError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0} already exists")]
    Conflict(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

impl From<executors::adapters::definition::DefinitionError> for AgentLibraryError {
    fn from(e: executors::adapters::definition::DefinitionError) -> Self {
        Self::Invalid(e.to_string())
    }
}

type Result<T> = std::result::Result<T, AgentLibraryError>;

#[derive(Debug, Clone, Serialize, TS)]
pub struct AgentSummary {
    pub name: String,
    /// `None` when the file doesn't parse; see `error`.
    pub definition: Option<AgentDefinition>,
    pub error: Option<String>,
    /// Referenced styles and skills that don't exist.
    pub missing: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct StyleSummary {
    pub name: String,
    pub body: String,
    pub used_by: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SkillSummary {
    pub name: String,
    pub description: Option<String>,
    pub file_count: i64,
    pub total_bytes: i64,
    pub error: Option<String>,
    pub used_by: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SkillFileInfo {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct ImportSummary {
    pub agents: Vec<String>,
    pub styles: Vec<String>,
    pub skills: Vec<String>,
    /// Names that replaced an existing item.
    pub replaced: Vec<String>,
    /// Files that were not imported, with the reason.
    pub skipped: Vec<String>,
}

pub struct AgentLibrary {
    root: PathBuf,
    pool: SqlitePool,
    sync_lock: Mutex<()>,
    watcher: StdMutex<Option<Debouncer<RecommendedWatcher, RecommendedCache>>>,
}

fn hash_bytes(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    format!("{:x}", hasher.finalize())
}

/// A relative path without `..`, root or drive components, `/`-separated.
fn safe_relative(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for component in Path::new(path.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A valid item name from a file or folder name.
pub fn slugify(raw: &str) -> String {
    let mut slug = String::new();
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if (c == '-' || c == '_' || c == ' ' || c == '.') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').chars().take(64).collect()
}

async fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("kanbain-tmp");
    tokio::fs::write(&tmp, contents).await?;
    tokio::fs::rename(&tmp, path).await
}

async fn read_dir_names(dir: &Path) -> std::io::Result<Vec<(String, PathBuf, bool)>> {
    let mut entries = Vec::new();
    let mut reader = match tokio::fs::read_dir(dir).await {
        Ok(reader) => reader,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(e) => return Err(e),
    };
    while let Some(entry) = reader.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".kanbain-tmp") {
            continue;
        }
        let is_dir = entry.file_type().await?.is_dir();
        entries.push((name, entry.path(), is_dir));
    }
    entries.sort();
    Ok(entries)
}

/// All regular files under `dir`, relative and sorted.
fn walk_files(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(std::io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        files.push((rel, entry.path().to_path_buf()));
    }
    files.sort();
    Ok(files)
}

impl AgentLibrary {
    pub fn new(root: impl Into<PathBuf>, pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            pool,
            sync_lock: Mutex::new(()),
            watcher: StdMutex::new(None),
        })
    }

    fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    fn styles_dir(&self) -> PathBuf {
        self.root.join("styles")
    }

    fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    fn agent_path(&self, name: &str) -> PathBuf {
        self.agents_dir().join(format!("{name}.yaml"))
    }

    fn style_path(&self, name: &str) -> PathBuf {
        self.styles_dir().join(format!("{name}.md"))
    }

    fn skill_path(&self, name: &str) -> PathBuf {
        self.skills_dir().join(name)
    }

    /// Create the folders, seed the default agents on first start, index
    /// everything and start watching for changes.
    pub async fn init(self: &Arc<Self>) -> Result<()> {
        if !self.agents_dir().exists() {
            tokio::fs::create_dir_all(self.agents_dir()).await?;
            for agent in default_agents() {
                write_atomic(&self.agent_path(&agent.name), agent.to_yaml()?.as_bytes()).await?;
            }
            tracing::info!(
                "Seeded the default agents into {}",
                self.agents_dir().display()
            );
        }
        tokio::fs::create_dir_all(self.styles_dir()).await?;
        tokio::fs::create_dir_all(self.skills_dir()).await?;
        self.sync().await?;
        if let Err(e) = self.watch() {
            tracing::warn!("Agent library watcher not started: {e}");
        }
        Ok(())
    }

    fn watch(self: &Arc<Self>) -> notify::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut debouncer = new_debouncer(
            Duration::from_millis(300),
            None,
            move |result: DebounceEventResult| {
                if result.is_ok() {
                    let _ = tx.send(());
                }
            },
        )?;
        debouncer.watch(&self.root, RecursiveMode::Recursive)?;
        *self.watcher.lock().unwrap() = Some(debouncer);
        let library = Arc::downgrade(self);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                while rx.try_recv().is_ok() {}
                let Some(library) = library.upgrade() else {
                    break;
                };
                if let Err(e) = library.sync().await {
                    tracing::warn!("Agent library sync failed: {e}");
                }
            }
        });
        Ok(())
    }

    /// Re-read every file and update the index.
    pub async fn sync(&self) -> Result<()> {
        let _guard = self.sync_lock.lock().await;
        let pool = &self.pool;

        let mut agents = Vec::new();
        for (file, path, is_dir) in read_dir_names(&self.agents_dir()).await? {
            let Some(stem) = file
                .strip_suffix(".yaml")
                .or_else(|| file.strip_suffix(".yml"))
            else {
                continue;
            };
            if is_dir {
                continue;
            }
            let bytes = tokio::fs::read(&path).await?;
            let hash = hash_bytes(&[&bytes]);
            let parsed = std::str::from_utf8(&bytes)
                .map_err(|e| AgentLibraryError::Invalid(e.to_string()))
                .and_then(|text| Ok(AgentDefinition::from_yaml(text)?))
                .and_then(|def| {
                    if def.name == stem {
                        Ok(def)
                    } else {
                        Err(AgentLibraryError::Invalid(format!(
                            "name `{}` doesn't match the file name `{file}`",
                            def.name
                        )))
                    }
                });
            match parsed {
                Ok(def) => {
                    let json = serde_json::to_string(&def).unwrap_or_default();
                    AgentDefinitionRow::upsert(pool, stem, Some(&json), None, &hash).await?
                }
                Err(e) => {
                    AgentDefinitionRow::upsert(pool, stem, None, Some(&e.to_string()), &hash)
                        .await?
                }
            }
            agents.push(stem.to_string());
        }
        AgentDefinitionRow::delete_except(pool, &agents).await?;

        let mut styles = Vec::new();
        for (file, path, is_dir) in read_dir_names(&self.styles_dir()).await? {
            let Some(stem) = file.strip_suffix(".md") else {
                continue;
            };
            if is_dir || validate_name(stem).is_err() {
                continue;
            }
            let body = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            let hash = hash_bytes(&[body.as_bytes()]);
            StyleBlockRow::upsert(pool, stem, &body, &hash).await?;
            styles.push(stem.to_string());
        }
        StyleBlockRow::delete_except(pool, &styles).await?;

        let mut skills = Vec::new();
        for (name, path, is_dir) in read_dir_names(&self.skills_dir()).await? {
            if !is_dir || validate_name(&name).is_err() {
                continue;
            }
            let files = {
                let path = path.clone();
                tokio::task::spawn_blocking(move || walk_files(&path))
                    .await
                    .map_err(std::io::Error::other)??
            };
            let mut parts: Vec<Vec<u8>> = Vec::new();
            let mut total = 0usize;
            let mut description = None;
            let mut error = None;
            for (rel, file_path) in &files {
                let bytes = tokio::fs::read(file_path).await?;
                total += bytes.len();
                if rel == "SKILL.md" {
                    let (_, desc) = skill_frontmatter(&String::from_utf8_lossy(&bytes));
                    description = desc;
                }
                parts.push(rel.as_bytes().to_vec());
                parts.push(bytes);
            }
            if !files.iter().any(|(rel, _)| rel == "SKILL.md") {
                error = Some("SKILL.md is missing".to_string());
            } else if description.is_none() {
                error = Some("SKILL.md has no `description` in its frontmatter".to_string());
            }
            if total > MAX_SKILL_BYTES {
                error = Some(format!(
                    "skill is larger than {} MB",
                    MAX_SKILL_BYTES / 1024 / 1024
                ));
            }
            let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
            SkillRow::upsert(
                pool,
                &name,
                description.as_deref(),
                files.len() as i64,
                total as i64,
                error.as_deref(),
                &hash_bytes(&refs),
            )
            .await?;
            skills.push(name);
        }
        SkillRow::delete_except(pool, &skills).await?;
        Ok(())
    }

    // ---- listing ----

    pub async fn list_agents(&self) -> Result<Vec<AgentSummary>> {
        let styles: BTreeSet<String> = StyleBlockRow::list(&self.pool)
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect();
        let skills: BTreeSet<String> = SkillRow::list(&self.pool)
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect();
        Ok(AgentDefinitionRow::list(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                let definition: Option<AgentDefinition> = row
                    .definition
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok());
                let mut missing = Vec::new();
                if let Some(def) = &definition {
                    for style in def.styles.iter().filter(|s| !styles.contains(*s)) {
                        missing.push(format!("style {style}"));
                    }
                    for skill in def.skills.iter().filter(|s| !skills.contains(*s)) {
                        missing.push(format!("skill {skill}"));
                    }
                }
                AgentSummary {
                    name: row.name,
                    definition,
                    error: row.error,
                    missing,
                    updated_at: row.updated_at,
                }
            })
            .collect())
    }

    async fn definitions(&self) -> Result<Vec<AgentDefinition>> {
        Ok(self
            .list_agents()
            .await?
            .into_iter()
            .filter_map(|a| a.definition)
            .collect())
    }

    pub async fn list_styles(&self) -> Result<Vec<StyleSummary>> {
        let agents = self.definitions().await?;
        Ok(StyleBlockRow::list(&self.pool)
            .await?
            .into_iter()
            .map(|row| StyleSummary {
                used_by: agents
                    .iter()
                    .filter(|a| a.styles.contains(&row.name))
                    .map(|a| a.name.clone())
                    .collect(),
                name: row.name,
                body: row.body,
                updated_at: row.updated_at,
            })
            .collect())
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillSummary>> {
        let agents = self.definitions().await?;
        Ok(SkillRow::list(&self.pool)
            .await?
            .into_iter()
            .map(|row| SkillSummary {
                used_by: agents
                    .iter()
                    .filter(|a| a.skills.contains(&row.name))
                    .map(|a| a.name.clone())
                    .collect(),
                name: row.name,
                description: row.description,
                file_count: row.file_count,
                total_bytes: row.total_bytes,
                error: row.error,
                updated_at: row.updated_at,
            })
            .collect())
    }

    // ---- agents ----

    pub async fn get_agent(&self, name: &str) -> Result<AgentDefinition> {
        validate_name(name)?;
        let text = match tokio::fs::read_to_string(self.agent_path(name)).await {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AgentLibraryError::NotFound(format!("agent {name}")));
            }
            Err(e) => return Err(e.into()),
        };
        let def = AgentDefinition::from_yaml(&text)?;
        Ok(def)
    }

    /// Write an agent definition; `previous` renames an existing one.
    pub async fn save_agent(&self, def: &AgentDefinition, previous: Option<&str>) -> Result<()> {
        def.validate()?;
        let renamed = previous.filter(|p| *p != def.name);
        if let Some(previous) = renamed {
            validate_name(previous)?;
            if self.agent_path(&def.name).exists() {
                return Err(AgentLibraryError::Conflict(format!("agent {}", def.name)));
            }
        }
        write_atomic(&self.agent_path(&def.name), def.to_yaml()?.as_bytes()).await?;
        if let Some(previous) = renamed {
            let _ = tokio::fs::remove_file(self.agent_path(previous)).await;
        }
        self.sync().await
    }

    pub async fn create_agent(&self, def: &AgentDefinition) -> Result<()> {
        if self.agent_path(&def.name).exists() {
            return Err(AgentLibraryError::Conflict(format!("agent {}", def.name)));
        }
        self.save_agent(def, None).await
    }

    pub async fn delete_agent(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        match tokio::fs::remove_file(self.agent_path(name)).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AgentLibraryError::NotFound(format!("agent {name}")));
            }
            Err(e) => return Err(e.into()),
        }
        self.sync().await
    }

    // ---- styles ----

    pub async fn get_style(&self, name: &str) -> Result<StyleBlock> {
        validate_name(name)?;
        match tokio::fs::read_to_string(self.style_path(name)).await {
            Ok(body) => Ok(StyleBlock {
                name: name.to_string(),
                body,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AgentLibraryError::NotFound(format!("style {name}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Write a style block; `previous` renames it and updates the agents
    /// that use it.
    pub async fn save_style(&self, style: &StyleBlock, previous: Option<&str>) -> Result<()> {
        validate_name(&style.name)?;
        let renamed = previous.filter(|p| *p != style.name);
        if renamed.is_some() && self.style_path(&style.name).exists() {
            return Err(AgentLibraryError::Conflict(format!("style {}", style.name)));
        }
        write_atomic(&self.style_path(&style.name), style.body.as_bytes()).await?;
        if let Some(previous) = renamed {
            validate_name(previous)?;
            let _ = tokio::fs::remove_file(self.style_path(previous)).await;
            for mut agent in self.definitions().await? {
                if let Some(slot) = agent.styles.iter_mut().find(|s| *s == previous) {
                    *slot = style.name.clone();
                    write_atomic(&self.agent_path(&agent.name), agent.to_yaml()?.as_bytes())
                        .await?;
                }
            }
        }
        self.sync().await
    }

    pub async fn delete_style(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let users: Vec<String> = self
            .definitions()
            .await?
            .into_iter()
            .filter(|a| a.styles.iter().any(|s| s == name))
            .map(|a| a.name)
            .collect();
        if !users.is_empty() {
            return Err(AgentLibraryError::Invalid(format!(
                "style {name} is used by {}",
                users.join(", ")
            )));
        }
        match tokio::fs::remove_file(self.style_path(name)).await {
            Ok(()) => self.sync().await,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AgentLibraryError::NotFound(format!("style {name}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    // ---- skills ----

    pub async fn skill_files(&self, name: &str) -> Result<Vec<SkillFileInfo>> {
        validate_name(name)?;
        let dir = self.skill_path(name);
        if !dir.is_dir() {
            return Err(AgentLibraryError::NotFound(format!("skill {name}")));
        }
        let files = tokio::task::spawn_blocking(move || walk_files(&dir))
            .await
            .map_err(std::io::Error::other)??;
        let mut infos = Vec::new();
        for (path, full) in files {
            let size = tokio::fs::metadata(&full).await?.len();
            infos.push(SkillFileInfo { path, size });
        }
        Ok(infos)
    }

    pub async fn read_skill_file(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        validate_name(name)?;
        let rel = safe_relative(path)
            .ok_or_else(|| AgentLibraryError::Invalid(format!("bad path {path}")))?;
        match tokio::fs::read(self.skill_path(name).join(&rel)).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AgentLibraryError::NotFound(format!("{name}/{rel}")))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn write_skill_file(&self, name: &str, path: &str, contents: &[u8]) -> Result<()> {
        validate_name(name)?;
        let rel = safe_relative(path)
            .ok_or_else(|| AgentLibraryError::Invalid(format!("bad path {path}")))?;
        let existing: u64 = match self.skill_files(name).await {
            Ok(files) => files.iter().filter(|f| f.path != rel).map(|f| f.size).sum(),
            Err(AgentLibraryError::NotFound(_)) => 0,
            Err(e) => return Err(e),
        };
        if existing as usize + contents.len() > MAX_SKILL_BYTES {
            return Err(AgentLibraryError::Invalid(format!(
                "skill {name} would be larger than {} MB",
                MAX_SKILL_BYTES / 1024 / 1024
            )));
        }
        write_atomic(&self.skill_path(name).join(&rel), contents).await?;
        self.sync().await
    }

    pub async fn delete_skill_file(&self, name: &str, path: &str) -> Result<()> {
        validate_name(name)?;
        let rel = safe_relative(path)
            .ok_or_else(|| AgentLibraryError::Invalid(format!("bad path {path}")))?;
        if rel == "SKILL.md" {
            return Err(AgentLibraryError::Invalid(
                "SKILL.md can't be removed; delete the skill instead".into(),
            ));
        }
        tokio::fs::remove_file(self.skill_path(name).join(&rel)).await?;
        self.sync().await
    }

    pub async fn delete_skill(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let dir = self.skill_path(name);
        if !dir.is_dir() {
            return Err(AgentLibraryError::NotFound(format!("skill {name}")));
        }
        tokio::fs::remove_dir_all(dir).await?;
        self.sync().await
    }

    async fn load_skill(&self, name: &str) -> Result<Skill> {
        let mut files = Vec::new();
        for info in self.skill_files(name).await? {
            files.push(SkillFile {
                contents: self.read_skill_file(name, &info.path).await?,
                path: info.path,
            });
        }
        Ok(Skill {
            name: name.to_string(),
            files,
        })
    }

    /// An agent with its styles and skills, ready for an adapter. Fails
    /// when a referenced style or skill is missing or broken.
    pub async fn resolve(&self, name: &str) -> Result<ResolvedAgent> {
        let definition = self.get_agent(name).await?;
        let mut styles = Vec::new();
        for style in &definition.styles {
            styles.push(self.get_style(style).await?);
        }
        let skill_rows = SkillRow::list(&self.pool).await?;
        let mut skills = Vec::new();
        for skill in &definition.skills {
            if let Some(error) = skill_rows
                .iter()
                .find(|row| &row.name == skill)
                .and_then(|row| row.error.clone())
            {
                return Err(AgentLibraryError::Invalid(format!(
                    "skill {skill}: {error}"
                )));
            }
            skills.push(self.load_skill(skill).await?);
        }
        Ok(ResolvedAgent {
            definition,
            styles,
            skills,
        })
    }

    // ---- import / export ----

    /// Import a zip archive (a library export, skill folders, or agent and
    /// style files). `file_name` names a lone skill without a frontmatter
    /// name.
    pub async fn import_zip(&self, bytes: &[u8], file_name: &str) -> Result<ImportSummary> {
        let entries = tokio::task::block_in_place(|| read_zip(bytes))?;
        let stem = file_name.trim_end_matches(".zip");
        self.import_files(entries, Some(stem)).await
    }

    /// Import uploaded files, each with its path relative to the chosen
    /// folder (or just its name).
    pub async fn import_files(
        &self,
        entries: Vec<(String, Vec<u8>)>,
        fallback_name: Option<&str>,
    ) -> Result<ImportSummary> {
        let total: usize = entries.iter().map(|(_, b)| b.len()).sum();
        if total > MAX_IMPORT_BYTES {
            return Err(AgentLibraryError::Invalid(format!(
                "import is larger than {} MB",
                MAX_IMPORT_BYTES / 1024 / 1024
            )));
        }
        let plan = plan_import(entries, fallback_name);
        let mut summary = ImportSummary {
            skipped: plan.skipped,
            ..Default::default()
        };

        for (name, text) in plan.agents {
            match AgentDefinition::from_yaml(&text) {
                Ok(mut def) => {
                    if validate_name(&def.name).is_err() {
                        def.name = name.clone();
                    }
                    if self.agent_path(&def.name).exists() {
                        summary.replaced.push(format!("agent {}", def.name));
                    }
                    write_atomic(&self.agent_path(&def.name), def.to_yaml()?.as_bytes()).await?;
                    summary.agents.push(def.name);
                }
                Err(e) => summary.skipped.push(format!("agent {name}: {e}")),
            }
        }
        for (name, body) in plan.styles {
            if self.style_path(&name).exists() {
                summary.replaced.push(format!("style {name}"));
            }
            write_atomic(&self.style_path(&name), body.as_bytes()).await?;
            summary.styles.push(name);
        }
        for (name, files) in plan.skills {
            let size: usize = files.iter().map(|(_, b)| b.len()).sum();
            if size > MAX_SKILL_BYTES {
                summary.skipped.push(format!(
                    "skill {name}: larger than {} MB",
                    MAX_SKILL_BYTES / 1024 / 1024
                ));
                continue;
            }
            let dir = self.skill_path(&name);
            if dir.exists() {
                summary.replaced.push(format!("skill {name}"));
                tokio::fs::remove_dir_all(&dir).await?;
            }
            for (rel, bytes) in files {
                write_atomic(&dir.join(rel), &bytes).await?;
            }
            summary.skills.push(name);
        }
        self.sync().await?;
        Ok(summary)
    }

    /// A zip in the library layout. With `agents`, only those agents and
    /// the styles and skills they use; otherwise everything.
    pub async fn export_zip(&self, agents: Option<&[String]>) -> Result<Vec<u8>> {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let (agent_names, style_names, skill_names) = match agents {
            Some(names) => {
                let mut styles = BTreeSet::new();
                let mut skills = BTreeSet::new();
                for name in names {
                    let def = self.get_agent(name).await?;
                    styles.extend(def.styles);
                    skills.extend(def.skills);
                }
                (names.to_vec(), styles, skills)
            }
            None => (
                AgentDefinitionRow::list(&self.pool)
                    .await?
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
                StyleBlockRow::list(&self.pool)
                    .await?
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
                SkillRow::list(&self.pool)
                    .await?
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
            ),
        };
        for name in &agent_names {
            validate_name(name)?;
            entries.push((
                format!("agents/{name}.yaml"),
                tokio::fs::read(self.agent_path(name)).await?,
            ));
        }
        for name in &style_names {
            if let Ok(style) = self.get_style(name).await {
                entries.push((format!("styles/{name}.md"), style.body.into_bytes()));
            }
        }
        for name in &skill_names {
            if let Ok(skill) = self.load_skill(name).await {
                for file in skill.files {
                    entries.push((format!("skills/{name}/{}", file.path), file.contents));
                }
            }
        }
        tokio::task::block_in_place(|| write_zip(&entries))
    }
}

fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = Vec::new();
    let mut total = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let Some(path) = file
            .enclosed_name()
            .and_then(|p| safe_relative(&p.to_string_lossy()))
        else {
            continue;
        };
        let mut contents = Vec::new();
        (&mut file)
            .take((MAX_IMPORT_BYTES - total + 1) as u64)
            .read_to_end(&mut contents)?;
        total += contents.len();
        if total > MAX_IMPORT_BYTES {
            return Err(AgentLibraryError::Invalid(format!(
                "archive is larger than {} MB unpacked",
                MAX_IMPORT_BYTES / 1024 / 1024
            )));
        }
        entries.push((path, contents));
    }
    Ok(entries)
}

fn write_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (path, contents) in entries {
        writer.start_file(path.as_str(), options)?;
        writer.write_all(contents)?;
    }
    Ok(writer.finish()?.into_inner())
}

#[derive(Debug, Default)]
struct ImportPlan {
    agents: Vec<(String, String)>,
    styles: Vec<(String, String)>,
    skills: BTreeMap<String, Vec<(String, Vec<u8>)>>,
    skipped: Vec<String>,
}

/// Sort imported files into agents, styles and skills.
fn plan_import(entries: Vec<(String, Vec<u8>)>, fallback_name: Option<&str>) -> ImportPlan {
    let mut plan = ImportPlan::default();
    let mut files: Vec<(String, Vec<u8>)> = entries
        .into_iter()
        .filter_map(|(path, bytes)| safe_relative(&path).map(|p| (p, bytes)))
        .filter(|(path, _)| {
            !path.starts_with("__MACOSX/")
                && !path
                    .split('/')
                    .any(|part| part == ".DS_Store" || part == ".git")
        })
        .collect();
    let mut fallback = fallback_name.map(slugify).filter(|s| !s.is_empty());

    // A single top folder that isn't the library layout: a skill folder if
    // it has SKILL.md, otherwise just a wrapper to strip.
    let first = |path: &str| path.split('/').next().unwrap_or("").to_string();
    if let Some((top, _)) = files.first()
        && top.contains('/')
    {
        let top = first(top);
        let layout = ["agents", "styles", "skills"].contains(&top.as_str());
        if !layout
            && files
                .iter()
                .all(|(p, _)| p.contains('/') && first(p) == top)
        {
            if !files.iter().any(|(p, _)| *p == format!("{top}/SKILL.md")) {
                fallback = Some(slugify(&top)).filter(|s| !s.is_empty()).or(fallback);
            } else {
                fallback = Some(slugify(&top));
            }
            files = files
                .into_iter()
                .map(|(p, b)| (p[top.len() + 1..].to_string(), b))
                .collect();
        }
    }

    let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).ok();
    let layout = files.iter().any(|(p, _)| {
        p.starts_with("agents/") || p.starts_with("styles/") || p.starts_with("skills/")
    });

    // Skill folders: wherever a SKILL.md is, outside agents/ and styles/.
    let skill_roots: Vec<String> = files
        .iter()
        .filter_map(|(p, _)| p.strip_suffix("SKILL.md"))
        .filter(|root| root.is_empty() || root.ends_with('/'))
        .map(|root| root.to_string())
        .filter(|root| !root.starts_with("agents/") && !root.starts_with("styles/"))
        .collect();

    for (path, bytes) in files {
        if let Some(root) = skill_roots
            .iter()
            .filter(|root| path.starts_with(root.as_str()))
            .max_by_key(|root| root.len())
        {
            let rel = path[root.len()..].to_string();
            let name = if root.is_empty() {
                let (front_name, _) =
                    skill_frontmatter(&String::from_utf8_lossy(match rel.as_str() {
                        "SKILL.md" => &bytes,
                        _ => &[],
                    }));
                // Decided once SKILL.md is seen; see below.
                front_name
                    .map(|n| slugify(&n))
                    .or_else(|| fallback.clone())
                    .unwrap_or_else(|| "skill".to_string())
            } else {
                slugify(root.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
            };
            plan.skills.entry(name).or_default().push((rel, bytes));
            continue;
        }
        let file_name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let in_agents = !layout || path.starts_with("agents/");
        let in_styles = !layout || path.starts_with("styles/");
        if in_agents && (file_name.ends_with(".yaml") || file_name.ends_with(".yml")) {
            match text(&bytes) {
                Some(t) => plan.agents.push((
                    slugify(file_name.trim_end_matches(".yaml").trim_end_matches(".yml")),
                    t,
                )),
                None => plan.skipped.push(format!("{path}: not UTF-8")),
            }
        } else if in_styles && file_name.ends_with(".md") {
            let name = slugify(file_name.trim_end_matches(".md"));
            match text(&bytes) {
                Some(t) if !name.is_empty() => plan.styles.push((name, t)),
                _ => plan.skipped.push(format!("{path}: not a style block")),
            }
        } else {
            plan.skipped
                .push(format!("{path}: not an agent, style or skill"));
        }
    }

    // A root-level skill's files may have been keyed before SKILL.md named
    // it; merge everything under the SKILL.md's name.
    if skill_roots.iter().any(|r| r.is_empty()) {
        let named = plan
            .skills
            .iter()
            .find(|(_, files)| files.iter().any(|(rel, _)| rel == "SKILL.md"))
            .map(|(name, _)| name.clone());
        if let Some(named) = named {
            let others: Vec<String> = plan
                .skills
                .keys()
                .filter(|k| **k != named)
                .filter(|k| !plan.skills[*k].iter().any(|(rel, _)| rel == "SKILL.md"))
                .cloned()
                .collect();
            for other in others {
                let files = plan.skills.remove(&other).unwrap_or_default();
                plan.skills.get_mut(&named).unwrap().extend(files);
            }
        }
    }
    plan.skills.retain(|name, _| validate_name(name).is_ok());
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn library() -> (tempfile::TempDir, Arc<AgentLibrary>) {
        let dir = tempfile::tempdir().unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let library = AgentLibrary::new(dir.path().join("lib"), pool);
        library.init().await.unwrap();
        (dir, library)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seeds_defaults_and_resolves() {
        let (_dir, library) = library().await;
        let agents = library.list_agents().await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            ["coder", "history", "intake", "planner", "scribe", "tester"]
        );
        library
            .save_style(
                &StyleBlock {
                    name: "concise".into(),
                    body: "Be concise.".into(),
                },
                None,
            )
            .await
            .unwrap();
        let mut coder = library.get_agent("coder").await.unwrap();
        coder.styles.push("concise".into());
        library.save_agent(&coder, Some("coder")).await.unwrap();
        let resolved = library.resolve("coder").await.unwrap();
        assert!(resolved.system_prompt().ends_with("Be concise."));

        // Renaming a style updates the agents using it.
        library
            .save_style(
                &StyleBlock {
                    name: "terse".into(),
                    body: "Be terse.".into(),
                },
                Some("concise"),
            )
            .await
            .unwrap();
        assert_eq!(library.get_agent("coder").await.unwrap().styles, ["terse"]);
        assert!(library.delete_style("terse").await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn indexes_broken_files_and_external_edits() {
        let (dir, library) = library().await;
        std::fs::write(dir.path().join("lib/agents/bad.yaml"), "name: [").unwrap();
        std::fs::write(
            dir.path().join("lib/agents/other.yaml"),
            "name: wrong\nrole: coder\nruntime: opencode\nmodel: sonnet\n",
        )
        .unwrap();
        library.sync().await.unwrap();
        let agents = library.list_agents().await.unwrap();
        let bad = agents.iter().find(|a| a.name == "bad").unwrap();
        assert!(bad.definition.is_none() && bad.error.is_some());
        let other = agents.iter().find(|a| a.name == "other").unwrap();
        assert!(other.error.as_deref().unwrap().contains("doesn't match"));
        std::fs::remove_file(dir.path().join("lib/agents/bad.yaml")).unwrap();
        library.sync().await.unwrap();
        assert!(
            !library
                .list_agents()
                .await
                .unwrap()
                .iter()
                .any(|a| a.name == "bad")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn imports_skill_folder_and_zip_roundtrip() {
        let (_dir, library) = library().await;
        let skill_md = b"---\nname: PDF Tools\ndescription: Work with PDFs\n---\n# PDF\n".to_vec();
        let summary = library
            .import_files(
                vec![
                    ("pdf-tools/SKILL.md".into(), skill_md.clone()),
                    ("pdf-tools/scripts/extract.py".into(), b"print(1)".to_vec()),
                    ("pdf-tools/../evil".into(), b"x".to_vec()),
                ],
                None,
            )
            .await
            .unwrap();
        assert_eq!(summary.skills, ["pdf-tools"]);
        let files = library.skill_files("pdf-tools").await.unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["SKILL.md", "scripts/extract.py"]);

        // A zip with SKILL.md at its root takes the frontmatter name.
        let zip = write_zip(&[
            ("SKILL.md".into(), skill_md),
            ("ref.md".into(), b"ref".to_vec()),
        ])
        .unwrap();
        let summary = library.import_zip(&zip, "download.zip").await.unwrap();
        assert_eq!(summary.skills, ["pdf-tools"]);
        assert_eq!(summary.replaced, ["skill pdf-tools"]);

        let mut coder = library.get_agent("coder").await.unwrap();
        coder.skills.push("pdf-tools".into());
        library.save_agent(&coder, None).await.unwrap();
        let resolved = library.resolve("coder").await.unwrap();
        assert_eq!(resolved.skills[0].files.len(), 2);

        // Export one agent with its skill, then import into a fresh library.
        let exported = library
            .export_zip(Some(&["coder".to_string()]))
            .await
            .unwrap();
        let (_dir2, other) = self::library().await;
        other.delete_agent("coder").await.unwrap();
        let summary = other.import_zip(&exported, "export.zip").await.unwrap();
        assert_eq!(summary.agents, ["coder"]);
        assert_eq!(summary.skills, ["pdf-tools"]);
        assert!(other.resolve("coder").await.is_ok());
    }

    #[test]
    fn plans_loose_files() {
        let plan = plan_import(
            vec![
                ("Backend Coder.yaml".into(), b"name: x".to_vec()),
                ("Concise Commits.md".into(), b"Use short commits.".to_vec()),
                ("notes.txt".into(), b"?".to_vec()),
            ],
            None,
        );
        assert_eq!(plan.agents[0].0, "backend-coder");
        assert_eq!(plan.styles[0].0, "concise-commits");
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(safe_relative("../x"), None);
        assert_eq!(safe_relative("/a/./b"), Some("a/b".into()));
    }
}

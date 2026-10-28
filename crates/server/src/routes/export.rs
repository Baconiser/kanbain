//! Full export of the local ticket data (plan D12), as JSON or Markdown.

use std::fmt::Write as _;

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use db::models::{
    issue::Issue,
    issue_comment::IssueComment,
    issue_relationship::IssueRelationship,
    project::{Project, ProjectRepo},
    project_status::ProjectStatus,
    project_tag::{IssueTag, ProjectTag},
    repo::Repo,
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use ts_rs::TS;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

#[derive(Debug, Clone, Copy, Default, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Json,
    Markdown,
}

#[derive(Debug, Deserialize, TS)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: ExportFormat,
    /// Limit the export to one project.
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct ExportedIssue {
    #[serde(flatten)]
    issue: Issue,
    internal_id: String,
    tags: Vec<String>,
    comments: Vec<IssueComment>,
}

#[derive(Debug, Serialize)]
struct ExportedProject {
    project: Project,
    repos: Vec<Repo>,
    statuses: Vec<ProjectStatus>,
    tags: Vec<ProjectTag>,
    relationships: Vec<IssueRelationship>,
    issues: Vec<ExportedIssue>,
}

#[derive(Debug, Serialize)]
struct Export {
    exported_at: DateTime<Utc>,
    projects: Vec<ExportedProject>,
}

async fn collect(pool: &SqlitePool, project_id: Option<Uuid>) -> Result<Export, sqlx::Error> {
    let projects = Project::find_all(pool)
        .await?
        .into_iter()
        .filter(|p| project_id.is_none_or(|id| p.id == id));

    let mut exported = Vec::new();
    for project in projects {
        let tags = ProjectTag::find_by_project(pool, project.id).await?;
        let issue_tags = IssueTag::find_by_project(pool, project.id).await?;
        let mut issues = Vec::new();
        for issue in Issue::find_by_project(pool, project.id).await? {
            let tag_names = issue_tags
                .iter()
                .filter(|it| it.issue_id == issue.id)
                .filter_map(|it| tags.iter().find(|t| t.id == it.tag_id))
                .map(|t| t.name.clone())
                .collect();
            issues.push(ExportedIssue {
                internal_id: issue.internal_id(&project.id_prefix),
                tags: tag_names,
                comments: IssueComment::find_by_issue(pool, issue.id).await?,
                issue,
            });
        }
        issues.sort_by_key(|i| i.issue.number);
        exported.push(ExportedProject {
            repos: ProjectRepo::find_repos_for_project(pool, project.id).await?,
            statuses: ProjectStatus::find_by_project(pool, project.id).await?,
            relationships: IssueRelationship::find_by_project(pool, project.id).await?,
            tags,
            issues,
            project,
        });
    }
    Ok(Export {
        exported_at: Utc::now(),
        projects: exported,
    })
}

fn render_markdown(export: &Export) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# KanbAIn export\n\nExported {}.\n",
        export.exported_at.format("%Y-%m-%d %H:%M UTC")
    );
    for p in &export.projects {
        let _ = writeln!(out, "## {}\n", p.project.name);
        if !p.repos.is_empty() {
            let names: Vec<&str> = p.repos.iter().map(|r| r.display_name.as_str()).collect();
            let _ = writeln!(out, "Repositories: {}\n", names.join(", "));
        }
        for status in &p.statuses {
            let in_status: Vec<&ExportedIssue> = p
                .issues
                .iter()
                .filter(|i| i.issue.status_id == status.id)
                .collect();
            if in_status.is_empty() {
                continue;
            }
            let _ = writeln!(out, "### {} ({})\n", status.name, in_status.len());
            for item in in_status {
                let issue = &item.issue;
                let _ = writeln!(out, "#### {}: {}\n", item.internal_id, issue.title);
                let mut meta = Vec::new();
                if let Some(external_id) = &issue.external_id {
                    meta.push(format!("External ID: {external_id}"));
                }
                if let Some(priority) = issue.priority {
                    meta.push(format!("Priority: {priority}"));
                }
                meta.push(format!("Mode: {}", issue.mode));
                if let Some(parent) = issue
                    .parent_issue_id
                    .and_then(|id| p.issues.iter().find(|i| i.issue.id == id))
                {
                    meta.push(format!("Parent: {}", parent.internal_id));
                }
                if !item.tags.is_empty() {
                    meta.push(format!("Tags: {}", item.tags.join(", ")));
                }
                let _ = writeln!(out, "- {}\n", meta.join("\n- "));
                if let Some(description) = &issue.description {
                    let _ = writeln!(out, "{}\n", description.trim());
                }
                if !item.comments.is_empty() {
                    let _ = writeln!(out, "**Comments**\n");
                    for c in &item.comments {
                        let _ = writeln!(
                            out,
                            "> **{}** ({}):\n> {}\n",
                            c.author,
                            c.created_at.format("%Y-%m-%d %H:%M"),
                            c.message.trim().replace('\n', "\n> ")
                        );
                    }
                }
            }
        }
    }
    out
}

async fn export(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let export = collect(&deployment.db().pool, query.project_id).await?;
    let stamp = export.exported_at.format("%Y%m%d-%H%M%S");
    let (body, content_type, ext) = match query.format {
        ExportFormat::Json => (
            serde_json::to_string_pretty(&export)
                .map_err(|e| ApiError::BadRequest(e.to_string()))?,
            "application/json",
            "json",
        ),
        ExportFormat::Markdown => (
            render_markdown(&export),
            "text/markdown; charset=utf-8",
            "md",
        ),
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"kanbain-export-{stamp}.{ext}\""),
            ),
        ],
        body,
    )
        .into_response())
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new().route("/export", get(export))
}

#[cfg(test)]
mod tests {
    use db::models::{
        issue::{IssueMode, IssuePriority},
        project::ProjectSettings,
        project_status::ProjectStatusKind,
    };
    use sqlx::types::Json;

    use super::*;

    #[test]
    fn markdown_groups_tickets_by_status() {
        let now = Utc::now();
        let project_id = Uuid::new_v4();
        let status = ProjectStatus {
            id: Uuid::new_v4(),
            project_id,
            name: "Backlog".into(),
            color: "0 0% 50%".into(),
            sort_order: 0,
            hidden: false,
            kind: ProjectStatusKind::Backlog,
            created_at: now,
        };
        let issue = Issue {
            id: Uuid::new_v4(),
            project_id,
            number: 7,
            status_id: status.id,
            title: "Login page".into(),
            description: Some("Users sign in.".into()),
            priority: Some(IssuePriority::High),
            sort_order: 1.0,
            parent_issue_id: None,
            parent_issue_sort_order: None,
            external_id: Some("PROJ-1".into()),
            mode: IssueMode::Ask,
            fix_round: 0,
            budget_usd: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        };
        let export = Export {
            exported_at: now,
            projects: vec![ExportedProject {
                project: Project {
                    id: project_id,
                    name: "Demo".into(),
                    id_prefix: "KAN".into(),
                    external_id_pattern: None,
                    external_id_key: None,
                    sort_order: 0,
                    settings: Json(ProjectSettings::default()),
                    created_at: now,
                    updated_at: now,
                },
                repos: vec![],
                statuses: vec![status],
                tags: vec![],
                relationships: vec![],
                issues: vec![ExportedIssue {
                    internal_id: "KAN-7".into(),
                    tags: vec!["ui".into()],
                    comments: vec![],
                    issue,
                }],
            }],
        };
        let md = render_markdown(&export);
        assert!(md.contains("## Demo"));
        assert!(md.contains("### Backlog (1)"));
        assert!(md.contains("#### KAN-7: Login page"));
        assert!(md.contains("External ID: PROJ-1"));
        assert!(md.contains("Priority: high"));
        assert!(md.contains("Tags: ui"));
        assert!(md.contains("Users sign in."));
    }
}

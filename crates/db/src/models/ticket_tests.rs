//! Tests for the local ticket models against an in-memory database with all
//! migrations applied.

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use super::{
    issue::{CreateIssue, Issue, IssueError, IssueMode, UpdateIssue},
    issue_comment::{CreateIssueComment, IssueComment, USER_AUTHOR},
    issue_relationship::{CreateIssueRelationship, IssueRelationship, IssueRelationshipType},
    project::{CreateProject, Project, UpdateProject},
    project_status::{DEFAULT_STATUSES, ProjectStatus, ProjectStatusKind},
    project_tag::{CreateProjectTag, IssueTag, ProjectTag},
};

async fn pool() -> SqlitePool {
    // One connection: every in-memory connection is its own database.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            "sqlite::memory:"
                .parse::<SqliteConnectOptions>()
                .unwrap()
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

async fn project(pool: &SqlitePool, prefix: Option<&str>) -> Project {
    Project::create(
        pool,
        &CreateProject {
            name: "Demo".to_string(),
            id_prefix: prefix.map(str::to_string),
            repo_ids: vec![],
        },
    )
    .await
    .unwrap()
}

fn new_issue(title: &str) -> CreateIssue {
    CreateIssue {
        title: title.to_string(),
        description: None,
        status_id: None,
        priority: None,
        sort_order: None,
        parent_issue_id: None,
        parent_issue_sort_order: None,
        external_id: None,
        mode: None,
        budget_usd: None,
    }
}

#[tokio::test]
async fn project_seeds_default_statuses_by_kind() {
    let pool = pool().await;
    let project = project(&pool, Some("web")).await;
    assert_eq!(project.id_prefix, "WEB");

    let statuses = ProjectStatus::find_by_project(&pool, project.id)
        .await
        .unwrap();
    assert_eq!(statuses.len(), DEFAULT_STATUSES.len());
    assert_eq!(statuses[0].kind, ProjectStatusKind::Backlog);
    assert!(
        statuses
            .iter()
            .any(|s| s.kind == ProjectStatusKind::Blocked)
    );
}

#[tokio::test]
async fn issues_get_sequential_numbers_and_backlog_status() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let backlog = ProjectStatus::find_first_of_kind(&pool, project.id, ProjectStatusKind::Backlog)
        .await
        .unwrap()
        .unwrap();

    let first = Issue::create(&pool, project.id, &new_issue("First"))
        .await
        .unwrap();
    let second = Issue::create(&pool, project.id, &new_issue("Second"))
        .await
        .unwrap();
    assert_eq!((first.number, second.number), (1, 2));
    assert_eq!(first.status_id, backlog.id);
    assert_eq!(first.mode, IssueMode::Ask);
    assert!(second.sort_order > first.sort_order);
    assert_eq!(second.internal_id(&project.id_prefix), "KAN-2");

    // Numbers are not reused after a delete.
    Issue::delete(&pool, second.id).await.unwrap();
    let third = Issue::create(&pool, project.id, &new_issue("Third"))
        .await
        .unwrap();
    assert_eq!(third.number, 3);
}

#[tokio::test]
async fn moving_to_done_sets_completed_at() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let done = ProjectStatus::find_first_of_kind(&pool, project.id, ProjectStatusKind::Done)
        .await
        .unwrap()
        .unwrap();
    let issue = Issue::create(&pool, project.id, &new_issue("Ship it"))
        .await
        .unwrap();
    assert!(issue.completed_at.is_none());

    let moved = Issue::update(
        &pool,
        issue.id,
        &UpdateIssue {
            status_id: Some(done.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(moved.completed_at.is_some());
}

#[tokio::test]
async fn update_clears_nullable_fields_and_keeps_others() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let mut data = new_issue("Title");
    data.description = Some("Body".to_string());
    data.external_id = Some(" PROJ-7 ".to_string());
    let issue = Issue::create(&pool, project.id, &data).await.unwrap();
    assert_eq!(issue.external_id.as_deref(), Some("PROJ-7"));
    assert_eq!(issue.display_id(&project.id_prefix), "PROJ-7");

    let updated = Issue::update(
        &pool,
        issue.id,
        &UpdateIssue {
            description: Some(None),
            mode: Some(IssueMode::Yolo),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.description, None);
    assert_eq!(updated.title, "Title");
    assert_eq!(updated.external_id.as_deref(), Some("PROJ-7"));
    assert_eq!(updated.mode, IssueMode::Yolo);

    let found = Issue::find_by_external_id(&pool, project.id, "proj-7")
        .await
        .unwrap();
    assert_eq!(found.map(|i| i.id), Some(issue.id));
}

#[tokio::test]
async fn parent_cycles_and_foreign_statuses_are_rejected() {
    let pool = pool().await;
    let project_a = project(&pool, None).await;
    let project_b = project(&pool, None).await;
    let parent = Issue::create(&pool, project_a.id, &new_issue("Parent"))
        .await
        .unwrap();
    let mut child_data = new_issue("Child");
    child_data.parent_issue_id = Some(parent.id);
    let child = Issue::create(&pool, project_a.id, &child_data)
        .await
        .unwrap();

    let cycle = Issue::update(
        &pool,
        parent.id,
        &UpdateIssue {
            parent_issue_id: Some(Some(child.id)),
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(cycle, Err(IssueError::ParentCycle)));

    let foreign_status = ProjectStatus::find_by_project(&pool, project_b.id)
        .await
        .unwrap()[0]
        .id;
    let invalid = Issue::update(
        &pool,
        child.id,
        &UpdateIssue {
            status_id: Some(foreign_status),
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(invalid, Err(IssueError::InvalidStatus)));
}

#[tokio::test]
async fn deleting_a_status_moves_its_issues() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let statuses = ProjectStatus::find_by_project(&pool, project.id)
        .await
        .unwrap();
    let issue = Issue::create(&pool, project.id, &new_issue("Move me"))
        .await
        .unwrap();
    assert_eq!(issue.status_id, statuses[0].id);

    ProjectStatus::delete(&pool, statuses[0].id, statuses[1].id)
        .await
        .unwrap();
    let issue = Issue::find_by_id(&pool, issue.id).await.unwrap().unwrap();
    assert_eq!(issue.status_id, statuses[1].id);
}

#[tokio::test]
async fn tags_relationships_and_comments_cascade_with_project() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let a = Issue::create(&pool, project.id, &new_issue("A"))
        .await
        .unwrap();
    let b = Issue::create(&pool, project.id, &new_issue("B"))
        .await
        .unwrap();

    let tag = ProjectTag::create(
        &pool,
        project.id,
        &CreateProjectTag {
            name: "bug".to_string(),
            color: "0 84% 60%".to_string(),
        },
    )
    .await
    .unwrap();
    let first = IssueTag::add(&pool, project.id, a.id, tag.id)
        .await
        .unwrap();
    let again = IssueTag::add(&pool, project.id, a.id, tag.id)
        .await
        .unwrap();
    assert_eq!(first.id, again.id);

    IssueRelationship::create(
        &pool,
        project.id,
        a.id,
        &CreateIssueRelationship {
            related_issue_id: b.id,
            relationship_type: IssueRelationshipType::Blocking,
        },
    )
    .await
    .unwrap();
    IssueComment::create(
        &pool,
        a.id,
        USER_AUTHOR,
        &CreateIssueComment {
            message: "Looks good".to_string(),
            parent_id: None,
        },
    )
    .await
    .unwrap();

    Project::delete(&pool, project.id).await.unwrap();
    assert!(
        IssueTag::find_by_project(&pool, project.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        IssueRelationship::find_by_project(&pool, project.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        IssueComment::find_by_issue(&pool, a.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        ProjectStatus::find_by_project(&pool, project.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn project_update_normalizes_fields() {
    let pool = pool().await;
    let project = project(&pool, None).await;
    let updated = Project::update(
        &pool,
        project.id,
        &UpdateProject {
            id_prefix: Some("ab-c".to_string()),
            external_id_key: Some(Some("  ".to_string())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.id_prefix, "ABC");
    assert_eq!(updated.external_id_key, None);
    assert_eq!(updated.name, "Demo");
}

#[tokio::test]
async fn description_versions_and_timeline() {
    use super::{
        issue_description::{DescriptionKind, IssueDescription, NewDescriptionVersion},
        issue_event::{IssueEvent, IssueEventType},
    };

    let pool = pool().await;
    let project = project(&pool, None).await;
    let mut data = new_issue("Login");
    data.description = Some("First".to_string());
    let issue = Issue::create(&pool, project.id, &data).await.unwrap();

    // Title-only and status changes add events but no version.
    let statuses = ProjectStatus::find_by_project(&pool, project.id)
        .await
        .unwrap();
    let in_progress = statuses
        .iter()
        .find(|s| s.kind == ProjectStatusKind::InProgress)
        .unwrap();
    Issue::update(
        &pool,
        issue.id,
        &UpdateIssue {
            title: Some("Login fails".to_string()),
            status_id: Some(in_progress.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // A sort-order move changes nothing on the timeline.
    Issue::update(
        &pool,
        issue.id,
        &UpdateIssue {
            sort_order: Some(5.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // A description edit becomes version 2.
    Issue::update(
        &pool,
        issue.id,
        &UpdateIssue {
            description: Some(Some("Second".to_string())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // An intake update forces version 3 with its texts.
    Issue::update_with_version(
        &pool,
        issue.id,
        &UpdateIssue::default(),
        Some(&NewDescriptionVersion {
            kind: DescriptionKind::Enhanced,
            original_text: Some("raw".to_string()),
            enhanced_text: Some("Second".to_string()),
            intake_id: None,
        }),
    )
    .await
    .unwrap();

    let versions = IssueDescription::find_by_issue(&pool, issue.id)
        .await
        .unwrap();
    let summary: Vec<_> = versions
        .iter()
        .map(|v| (v.version, v.kind, v.description.as_deref()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (3, DescriptionKind::Enhanced, Some("Second")),
            (2, DescriptionKind::Edited, Some("Second")),
            (1, DescriptionKind::Original, Some("First")),
        ]
    );
    assert_eq!(versions[0].original_text.as_deref(), Some("raw"));

    IssueComment::create(
        &pool,
        issue.id,
        USER_AUTHOR,
        &CreateIssueComment {
            message: "Looks bad".to_string(),
            parent_id: None,
        },
    )
    .await
    .unwrap();

    let events = IssueEvent::find_by_issue(&pool, issue.id).await.unwrap();
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            IssueEventType::Created,
            IssueEventType::DescriptionVersion,
            IssueEventType::TitleChanged,
            IssueEventType::StatusChanged,
            IssueEventType::DescriptionVersion,
            IssueEventType::DescriptionVersion,
            IssueEventType::Comment,
        ]
    );
    let status = &events[3].payload.0;
    assert_eq!(status["from"], "Backlog");
    assert_eq!(status["to_kind"], "in_progress");
    assert_eq!(events[0].payload.0["title"], "Login");
    assert_eq!(events[6].payload.0["excerpt"], "Looks bad");
    // Ids in trigger payloads are canonical UUIDs.
    let comment_id = events[6].payload.0["comment_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(comment_id).is_ok(), "{comment_id}");
    assert!(comment_id.contains('-'));

    // An event from before the ticket existed sorts first.
    let old = chrono::Utc::now() - chrono::Duration::days(400);
    IssueEvent::create(
        &pool,
        issue.id,
        IssueEventType::PriorCommit,
        "system",
        &serde_json::json!({"sha": "abc"}),
        Some(old),
    )
    .await
    .unwrap();
    let events = IssueEvent::find_by_issue(&pool, issue.id).await.unwrap();
    assert_eq!(events[0].event_type, IssueEventType::PriorCommit);
    assert!(
        IssueEvent::exists_with(&pool, issue.id, IssueEventType::PriorCommit, "sha", "abc")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn history_commits_and_file_links() {
    use super::{
        issue_history::{
            HistoryPermission, IssueHistory, NewCommitFile, NewTicketCommit, TicketCommit,
            TicketFileChange, TicketFileLink,
        },
        repo::Repo,
    };

    let pool = pool().await;
    let project = project(&pool, None).await;
    let issue = Issue::create(&pool, project.id, &new_issue("Reset"))
        .await
        .unwrap();
    let repo = Repo::find_or_create(&pool, std::path::Path::new("/tmp/shop"), "shop")
        .await
        .unwrap();
    let commit = |sha: &str, days: i64, files: Vec<(&str, i64)>| NewTicketCommit {
        sha: sha.to_string(),
        author: "Anna".to_string(),
        committed_at: chrono::Utc::now() - chrono::Duration::days(days),
        subject: format!("PROJ-1: {sha}"),
        branches: vec![],
        message_match: true,
        files: files
            .into_iter()
            .map(|(p, n)| NewCommitFile {
                path: p.to_string(),
                additions: Some(n),
                deletions: Some(1),
                hunks: vec![(1, n)],
            })
            .collect(),
    };
    TicketCommit::replace_for_repo(
        &pool,
        issue.id,
        repo.id,
        &[
            commit("aaa", 3, vec![("mailer.rs", 5), ("README.md", 1)]),
            commit("bbb", 1, vec![("mailer.rs", 2)]),
        ],
    )
    .await
    .unwrap();

    let commits = TicketCommit::find_by_issue(&pool, issue.id).await.unwrap();
    assert_eq!(
        commits.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        vec!["bbb", "aaa"]
    );
    assert_eq!(commits[0].repo_name, "shop");
    let files = TicketFileLink::find_by_issue(&pool, issue.id)
        .await
        .unwrap();
    assert_eq!(files[0].path, "mailer.rs");
    assert_eq!(
        (files[0].commits, files[0].additions, files[0].deletions),
        (2, 7, 2)
    );
    assert_eq!(files[0].last_commit_sha, "bbb");
    assert_eq!(files[1].path, "README.md");
    let changes = TicketFileChange::find_by_issue(&pool, issue.id)
        .await
        .unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].commit_sha, "bbb");
    assert_eq!(changes[0].hunks.0, vec![(1, 2)]);
    assert_eq!(
        TicketFileChange::issues_for_path(&pool, repo.id, "mailer.rs")
            .await
            .unwrap()
            .len(),
        2
    );

    // A re-scan replaces the repo's rows.
    TicketCommit::replace_for_repo(&pool, issue.id, repo.id, &[commit("ccc", 0, vec![])])
        .await
        .unwrap();
    assert_eq!(
        TicketCommit::find_by_issue(&pool, issue.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        TicketFileLink::find_by_issue(&pool, issue.id)
            .await
            .unwrap()
            .is_empty()
    );

    // Permission and summary survive a new check; new prior work is noticed.
    let h = IssueHistory::record_check(&pool, issue.id, "PROJ-1", 1, 1, "h1")
        .await
        .unwrap();
    assert_eq!(h.permission, HistoryPermission::Pending);
    assert!(h.needs_analysis());
    IssueHistory::set_permission(&pool, issue.id, HistoryPermission::Allowed)
        .await
        .unwrap();
    assert!(IssueHistory::start_analysis(&pool, issue.id).await.unwrap());
    assert!(!IssueHistory::start_analysis(&pool, issue.id).await.unwrap());
    IssueHistory::finish_analysis(&pool, issue.id, None, Ok(("summary", "h1")))
        .await
        .unwrap();
    let h = IssueHistory::record_check(&pool, issue.id, "PROJ-1", 1, 1, "h1")
        .await
        .unwrap();
    assert_eq!(h.permission, HistoryPermission::Allowed);
    assert_eq!(h.prior_work.as_deref(), Some("summary"));
    assert!(!h.needs_analysis());
    let h = IssueHistory::record_check(&pool, issue.id, "PROJ-1", 2, 2, "h2")
        .await
        .unwrap();
    assert!(h.needs_analysis());
}

#[tokio::test]
async fn plans_get_versions_and_planning_is_claimed_once() {
    use super::plan::{IssuePlanning, Plan, PlanDocument, PlanSource, PlanStatus, PlanningStatus};

    let pool = pool().await;
    let project = project(&pool, None).await;
    let issue = Issue::create(&pool, project.id, &new_issue("Plan me"))
        .await
        .unwrap();
    let doc = PlanDocument::from_value(serde_json::json!({
        "subtasks": [{"id": "1", "title": "Do it"}]
    }))
    .unwrap();

    let v1 = Plan::create(&pool, issue.id, &doc, PlanSource::Planner, None)
        .await
        .unwrap();
    let v2 = Plan::create(&pool, issue.id, &doc, PlanSource::Edited, None)
        .await
        .unwrap();
    assert_eq!((v1.version, v2.version), (1, 2));
    assert_eq!(v1.status, PlanStatus::Draft);
    Plan::decide(&pool, v2.id, PlanStatus::Rejected, Some("Too big"))
        .await
        .unwrap();
    let latest = Plan::latest(&pool, issue.id).await.unwrap().unwrap();
    assert_eq!(latest.status, PlanStatus::Rejected);
    assert_eq!(latest.feedback.as_deref(), Some("Too big"));
    assert!(latest.decided_at.is_some());

    // One planner run at a time per ticket.
    assert!(IssuePlanning::begin(&pool, issue.id).await.unwrap());
    assert!(!IssuePlanning::begin(&pool, issue.id).await.unwrap());
    IssuePlanning::set_status(&pool, issue.id, PlanningStatus::WaitingForAnswer, None)
        .await
        .unwrap();
    assert!(IssuePlanning::begin(&pool, issue.id).await.unwrap());
    assert_eq!(IssuePlanning::fail_interrupted(&pool).await.unwrap(), 1);
    let planning = IssuePlanning::find(&pool, issue.id).await.unwrap().unwrap();
    assert_eq!(planning.status, PlanningStatus::Failed);

    // Plans go with their ticket.
    Issue::delete(&pool, issue.id).await.unwrap();
    assert!(
        Plan::find_by_issue(&pool, issue.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn orchestration_claims_conflicts_and_restart_requeue() {
    use super::{
        orchestration::{
            OrchestrationStatus, OrchestrationSubtask, SubtaskConflict, SubtaskStatus,
            TicketOrchestration,
        },
        plan::{Plan, PlanDocument, PlanSource},
        workspace::{CreateWorkspace, Workspace},
    };

    let pool = pool().await;
    let project = project(&pool, None).await;
    let issue = Issue::create(&pool, project.id, &new_issue("Code me"))
        .await
        .unwrap();
    let doc = PlanDocument::from_value(serde_json::json!({
        "subtasks": [{"id": "1", "title": "A"}, {"id": "2", "title": "B"}]
    }))
    .unwrap();
    let plan = Plan::create(&pool, issue.id, &doc, PlanSource::Planner, None)
        .await
        .unwrap();
    let workspace = Workspace::create(
        &pool,
        &CreateWorkspace {
            branch: "kanbain/KAN-1-code-me".into(),
            name: None,
            issue_id: Some(issue.id),
        },
        uuid::Uuid::new_v4(),
    )
    .await
    .unwrap();
    TicketOrchestration::upsert(&pool, issue.id, plan.id, workspace.id)
        .await
        .unwrap();
    let a = OrchestrationSubtask::create(&pool, issue.id, "1", 0, None)
        .await
        .unwrap();
    let b = OrchestrationSubtask::create(&pool, issue.id, "2", 1, None)
        .await
        .unwrap();
    assert_eq!(a.status, SubtaskStatus::Pending);

    // A subtask is claimed once.
    assert!(OrchestrationSubtask::claim(&pool, a.id).await.unwrap());
    assert!(!OrchestrationSubtask::claim(&pool, a.id).await.unwrap());

    // A conflict queues the re-run with its context.
    let conflict = SubtaskConflict {
        files: vec!["shop/src/mail.rs".into()],
        previous_diff: "+x".into(),
    };
    OrchestrationSubtask::set_conflict(&pool, a.id, &conflict)
        .await
        .unwrap();
    let a = OrchestrationSubtask::find_by_id(&pool, a.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((a.status, a.fix_round), (SubtaskStatus::Pending, 1));
    assert_eq!(a.conflict.unwrap().0, conflict);

    let mut commits = std::collections::BTreeMap::new();
    commits.insert("shop".to_string(), "abc".to_string());
    OrchestrationSubtask::set_merged(&pool, a.id, &commits)
        .await
        .unwrap();

    // Restart: a running ticket's running subtask goes back to pending.
    assert!(OrchestrationSubtask::claim(&pool, b.id).await.unwrap());
    assert_eq!(
        OrchestrationSubtask::requeue_interrupted(&pool)
            .await
            .unwrap(),
        1
    );
    let subtasks = OrchestrationSubtask::find_by_issue(&pool, issue.id)
        .await
        .unwrap();
    assert_eq!(
        subtasks.iter().map(|s| s.status).collect::<Vec<_>>(),
        [SubtaskStatus::Merged, SubtaskStatus::Pending]
    );
    assert_eq!(subtasks[0].merge_commits.as_ref().unwrap().0["shop"], "abc");
    assert_eq!(
        TicketOrchestration::find_running(&pool).await.unwrap(),
        [issue.id]
    );

    // A stopped ticket's interrupted run stays stopped.
    assert!(OrchestrationSubtask::claim(&pool, b.id).await.unwrap());
    TicketOrchestration::set_status(&pool, issue.id, OrchestrationStatus::Stopped, None)
        .await
        .unwrap();
    assert_eq!(
        OrchestrationSubtask::requeue_interrupted(&pool)
            .await
            .unwrap(),
        0
    );
    let b = OrchestrationSubtask::find_by_id(&pool, b.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.status, SubtaskStatus::Stopped);
    assert!(OrchestrationSubtask::requeue(&pool, b.id).await.unwrap());

    Issue::set_fix_round(&pool, issue.id, 2).await.unwrap();
    assert_eq!(
        Issue::find_by_id(&pool, issue.id)
            .await
            .unwrap()
            .unwrap()
            .fix_round,
        2
    );
}

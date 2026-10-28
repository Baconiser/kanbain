use axum::{
    Json, Router,
    extract::{Path, State},
    response::{IntoResponse, Json as ResponseJson},
    routing::{delete, get, post, put},
};
use db::models::{
    attachment_text::AttachmentText,
    file::{File, IssueAttachment},
    issue::{Issue, UpdateIssue},
    issue_comment::{CreateIssueComment, IssueComment, USER_AUTHOR, UpdateIssueComment},
    issue_description::IssueDescription,
    issue_event::IssueEvent,
    issue_relationship::{CreateIssueRelationship, IssueRelationship},
    project_tag::{IssueTag, ProjectTag},
};
use deployment::Deployment;
use serde::Deserialize;
use ts_rs::TS;
use utils::{
    diff::{TextDiffLine, text_diff_lines},
    response::ApiResponse,
};
use uuid::Uuid;

use super::{attachments::AttachmentResponse, projects::forward_patch_stream};
use crate::{DeploymentImpl, error::ApiError, middleware::ws::WsUpgrade};

async fn load_issue(deployment: &DeploymentImpl, id: Uuid) -> Result<Issue, ApiError> {
    Issue::find_by_id(&deployment.db().pool, id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))
}

// --- Issues -----------------------------------------------------------------

async fn get_issue(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Issue>>, ApiError> {
    let issue = load_issue(&deployment, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(issue)))
}

async fn update_issue(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<UpdateIssue>,
) -> Result<ResponseJson<ApiResponse<Issue>>, ApiError> {
    let before = load_issue(&deployment, issue_id).await?;
    let issue = Issue::update(&deployment.db().pool, issue_id, &payload).await?;
    // A new External ID means different commits (D18).
    if issue.external_id != before.external_id {
        deployment.history().spawn_check(issue.id);
    }
    Ok(ResponseJson(ApiResponse::success(issue)))
}

async fn delete_issue(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    if Issue::delete(&deployment.db().pool, issue_id).await? == 0 {
        return Err(ApiError::Database(sqlx::Error::RowNotFound));
    }
    Ok(ResponseJson(ApiResponse::success(())))
}

// --- Tags -------------------------------------------------------------------

async fn add_issue_tag(
    Path((issue_id, tag_id)): Path<(Uuid, Uuid)>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssueTag>>, ApiError> {
    let pool = &deployment.db().pool;
    let issue = load_issue(&deployment, issue_id).await?;
    match ProjectTag::find_by_id(pool, tag_id).await? {
        Some(tag) if tag.project_id == issue.project_id => {}
        _ => {
            return Err(ApiError::BadRequest(
                "Tag does not belong to this project".to_string(),
            ));
        }
    }
    let issue_tag = IssueTag::add(pool, issue.project_id, issue_id, tag_id).await?;
    Ok(ResponseJson(ApiResponse::success(issue_tag)))
}

async fn remove_issue_tag(
    Path((issue_id, tag_id)): Path<(Uuid, Uuid)>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    IssueTag::remove(&deployment.db().pool, issue_id, tag_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

// --- Relationships ----------------------------------------------------------

async fn create_relationship(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateIssueRelationship>,
) -> Result<ResponseJson<ApiResponse<IssueRelationship>>, ApiError> {
    let issue = load_issue(&deployment, issue_id).await?;
    if payload.related_issue_id == issue_id {
        return Err(ApiError::BadRequest(
            "A ticket cannot relate to itself".to_string(),
        ));
    }
    let related = load_issue(&deployment, payload.related_issue_id).await?;
    if related.project_id != issue.project_id {
        return Err(ApiError::BadRequest(
            "Related ticket must be in the same project".to_string(),
        ));
    }
    let relationship =
        IssueRelationship::create(&deployment.db().pool, issue.project_id, issue_id, &payload)
            .await?;
    Ok(ResponseJson(ApiResponse::success(relationship)))
}

async fn delete_relationship(
    Path(relationship_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    IssueRelationship::delete(&deployment.db().pool, relationship_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

// --- Comments ---------------------------------------------------------------

async fn list_comments(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<IssueComment>>>, ApiError> {
    let comments = IssueComment::find_by_issue(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(comments)))
}

async fn create_comment(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateIssueComment>,
) -> Result<ResponseJson<ApiResponse<IssueComment>>, ApiError> {
    if payload.message.trim().is_empty() {
        return Err(ApiError::BadRequest("Comment is empty".to_string()));
    }
    load_issue(&deployment, issue_id).await?;
    let comment =
        IssueComment::create(&deployment.db().pool, issue_id, USER_AUTHOR, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(comment)))
}

async fn update_comment(
    Path(comment_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<UpdateIssueComment>,
) -> Result<ResponseJson<ApiResponse<IssueComment>>, ApiError> {
    let pool = &deployment.db().pool;
    let comment = IssueComment::find_by_id(pool, comment_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    if comment.author != USER_AUTHOR {
        return Err(ApiError::Forbidden(
            "Agent comments cannot be edited".to_string(),
        ));
    }
    if payload.message.trim().is_empty() {
        return Err(ApiError::BadRequest("Comment is empty".to_string()));
    }
    let comment = IssueComment::update(pool, comment_id, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(comment)))
}

async fn delete_comment(
    Path(comment_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    IssueComment::delete(&deployment.db().pool, comment_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn stream_comments_ws(
    ws: WsUpgrade,
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        match deployment
            .events()
            .stream_issue_comments_raw(issue_id)
            .await
        {
            Ok(stream) => forward_patch_stream(socket, stream).await,
            Err(e) => tracing::warn!("issue comments WS failed: {}", e),
        }
    })
}

// --- Attachments ------------------------------------------------------------

#[derive(Debug, Deserialize, TS)]
pub struct AssociateIssueAttachmentsRequest {
    pub attachment_ids: Vec<Uuid>,
}

async fn list_attachments(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<AttachmentResponse>>>, ApiError> {
    let files = File::find_by_issue_id(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        files
            .into_iter()
            .map(AttachmentResponse::from_file)
            .collect(),
    )))
}

async fn associate_attachments(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<AssociateIssueAttachmentsRequest>,
) -> Result<ResponseJson<ApiResponse<Vec<AttachmentResponse>>>, ApiError> {
    let pool = &deployment.db().pool;
    load_issue(&deployment, issue_id).await?;
    for id in &payload.attachment_ids {
        if File::find_by_id(pool, *id).await?.is_none() {
            return Err(ApiError::BadRequest(format!("Attachment {id} not found")));
        }
    }
    IssueAttachment::associate_many(pool, issue_id, &payload.attachment_ids).await?;
    let files = File::find_by_issue_id(pool, issue_id).await?;
    // Extract text now so the ticket shows it and search can index it.
    for file in files
        .iter()
        .filter(|f| payload.attachment_ids.contains(&f.id))
    {
        if let Err(e) = deployment.file().ensure_text(file).await {
            tracing::warn!("Extracting {} failed: {e}", file.original_name);
        }
    }
    Ok(ResponseJson(ApiResponse::success(
        files
            .into_iter()
            .map(AttachmentResponse::from_file)
            .collect(),
    )))
}

async fn remove_attachment(
    Path((issue_id, attachment_id)): Path<(Uuid, Uuid)>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    IssueAttachment::remove(&deployment.db().pool, issue_id, attachment_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn list_attachment_texts(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<AttachmentText>>>, ApiError> {
    let texts = AttachmentText::find_by_issue(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(texts)))
}

// --- Description versions and timeline ---------------------------------------

async fn list_descriptions(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<IssueDescription>>>, ApiError> {
    let versions = IssueDescription::find_by_issue(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(versions)))
}

async fn list_timeline(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<IssueEvent>>>, ApiError> {
    let events = IssueEvent::find_by_issue(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(events)))
}

async fn stream_timeline_ws(
    ws: WsUpgrade,
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        match deployment
            .events()
            .stream_issue_timeline_raw(issue_id)
            .await
        {
            Ok(stream) => forward_patch_stream(socket, stream).await,
            Err(e) => tracing::warn!("issue timeline WS failed: {}", e),
        }
    })
}

#[derive(Debug, Deserialize, TS)]
pub struct TextDiffRequest {
    pub old: String,
    pub new: String,
}

/// Line diff of two texts (description versions, intake updates).
async fn text_diff(
    Json(payload): Json<TextDiffRequest>,
) -> ResponseJson<ApiResponse<Vec<TextDiffLine>>> {
    ResponseJson(ApiResponse::success(text_diff_lines(
        &payload.old,
        &payload.new,
    )))
}

pub fn router() -> Router<DeploymentImpl> {
    let issue = Router::new()
        .route("/", get(get_issue).put(update_issue).delete(delete_issue))
        .route(
            "/tags/{tag_id}",
            post(add_issue_tag).delete(remove_issue_tag),
        )
        .route("/relationships", post(create_relationship))
        .route("/comments", get(list_comments).post(create_comment))
        .route("/comments/stream/ws", get(stream_comments_ws))
        .route(
            "/attachments",
            get(list_attachments).post(associate_attachments),
        )
        .route("/attachments/{attachment_id}", delete(remove_attachment))
        .route("/attachment-texts", get(list_attachment_texts))
        .route("/descriptions", get(list_descriptions))
        .route("/timeline", get(list_timeline))
        .route("/timeline/stream/ws", get(stream_timeline_ws));

    Router::new()
        .nest("/issues/{issue_id}", issue)
        .route("/text-diff", post(text_diff))
        .route(
            "/issue-relationships/{relationship_id}",
            delete(delete_relationship),
        )
        .route(
            "/issue-comments/{comment_id}",
            put(update_comment).delete(delete_comment),
        )
}

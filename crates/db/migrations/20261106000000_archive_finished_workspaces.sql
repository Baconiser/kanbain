-- Tester and fixer workspaces were never archived, and moving a ticket to
-- Done archived nothing. Archive what is left over: every unpinned workspace
-- of a main ticket in Done, of a passed checkpoint, or of a merged subtask.
UPDATE workspaces
SET archived = 1, updated_at = datetime('now', 'subsec')
WHERE archived = 0
  AND pinned = 0
  AND (
      id IN (SELECT tester_workspace_id FROM orchestration_checkpoints
              WHERE status = 'passed' AND tester_workspace_id IS NOT NULL)
   OR id IN (SELECT fixer_workspace_id FROM orchestration_checkpoints
              WHERE status = 'passed' AND fixer_workspace_id IS NOT NULL)
   OR id IN (SELECT workspace_id FROM orchestration_subtasks
              WHERE status = 'merged' AND workspace_id IS NOT NULL)
   OR issue_id IN (
          SELECT i.id FROM issues i
          JOIN project_statuses ps ON ps.id = COALESCE(
              (SELECT p.status_id FROM issues p WHERE p.id = i.parent_issue_id),
              i.status_id)
          WHERE ps.kind = 'done')
  );

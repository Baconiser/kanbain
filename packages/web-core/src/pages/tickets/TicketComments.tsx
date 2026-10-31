import { useState } from 'react';
import { Button } from '@kanbain/ui/components/Button';
import { Textarea } from '@kanbain/ui/components/Textarea';
import type { IssueComment } from 'shared/types';
import { useIssueComments } from '@/shared/hooks/useTicketStreams';
import { issuesApi } from '@/shared/lib/ticketsApi';
import { CollapsibleSection } from './CollapsibleSection';

interface TicketCommentsProps {
  issueId: string;
  onError: (message: string) => void;
}

// Long threads show the latest few until expanded.
const COMMENTS_SHOWN = 5;

function authorLabel(author: string): string {
  if (author === 'user') return 'You';
  if (author.startsWith('agent:')) return 'Agent';
  return author;
}

function CommentItem({
  comment,
  onError,
}: {
  comment: IssueComment;
  onError: (message: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(comment.message);
  const isUser = comment.author === 'user';

  const save = async () => {
    if (!draft.trim()) return;
    try {
      await issuesApi.updateComment(comment.id, draft);
      setEditing(false);
    } catch (err) {
      onError(err instanceof Error ? err.message : 'Failed to save comment');
    }
  };

  return (
    <li
      className={
        isUser
          ? 'rounded-sm border bg-secondary p-base'
          : 'rounded-sm border border-brand/40 bg-brand/10 p-base'
      }
    >
      <div className="mb-half flex items-center justify-between gap-base text-xs text-low">
        <span className="font-medium text-normal">
          {authorLabel(comment.author)}
        </span>
        <span>{new Date(comment.created_at).toLocaleString()}</span>
      </div>
      {editing ? (
        <div className="flex flex-col gap-half">
          <Textarea
            aria-label="Edit comment"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <div className="flex gap-half">
            <Button size="sm" onClick={save}>
              Save
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setDraft(comment.message);
                setEditing(false);
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <p className="whitespace-pre-wrap text-sm">{comment.message}</p>
      )}
      {isUser && !editing && (
        <div className="mt-half flex gap-base text-xs text-low">
          <button
            type="button"
            className="hover:text-normal"
            onClick={() => setEditing(true)}
          >
            Edit
          </button>
          <button
            type="button"
            className="hover:text-error"
            onClick={() =>
              issuesApi
                .deleteComment(comment.id)
                .catch((err: unknown) =>
                  onError(
                    err instanceof Error
                      ? err.message
                      : 'Failed to delete comment'
                  )
                )
            }
          >
            Delete
          </button>
        </div>
      )}
    </li>
  );
}

export function TicketComments({ issueId, onError }: TicketCommentsProps) {
  const { comments, isLoading } = useIssueComments(issueId);
  const [message, setMessage] = useState('');
  const [sending, setSending] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const hidden = showAll ? 0 : Math.max(0, comments.length - COMMENTS_SHOWN);

  const send = async () => {
    if (!message.trim()) return;
    setSending(true);
    try {
      await issuesApi.createComment(issueId, { message });
      setMessage('');
    } catch (err) {
      onError(err instanceof Error ? err.message : 'Failed to add comment');
    } finally {
      setSending(false);
    }
  };

  return (
    <CollapsibleSection
      id="comments"
      title="Comments"
      meta={comments.length}
      testId="ticket-comments"
    >
      {isLoading ? (
        <p className="text-sm text-low">Loading…</p>
      ) : (
        <ul className="flex flex-col gap-half">
          {hidden > 0 && (
            <li>
              <button
                type="button"
                className="text-xs text-low hover:text-normal"
                onClick={() => setShowAll(true)}
              >
                Show {hidden} earlier comment{hidden === 1 ? '' : 's'}
              </button>
            </li>
          )}
          {comments.slice(hidden).map((c) => (
            <CommentItem key={c.id} comment={c} onError={onError} />
          ))}
        </ul>
      )}
      <Textarea
        aria-label="New comment"
        placeholder="Write a comment… (Ctrl+Enter to send)"
        rows={2}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void send();
          }
        }}
      />
      <div className="flex justify-end">
        <Button size="xs" onClick={send} disabled={sending || !message.trim()}>
          Comment
        </Button>
      </div>
    </CollapsibleSection>
  );
}

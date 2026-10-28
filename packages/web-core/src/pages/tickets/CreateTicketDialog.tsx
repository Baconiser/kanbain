import { useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { Label } from '@kanbain/ui/components/Label';
import { Textarea } from '@kanbain/ui/components/Textarea';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@kanbain/ui/components/KeyboardDialog';
import type { Issue, IssueMode } from 'shared/types';
import { defineModal, getErrorMessage } from '@/shared/lib/modals';
import { projectsApi } from '@/shared/lib/ticketsApi';

export interface CreateTicketDialogProps {
  projectId: string;
  statusId?: string;
}

const CreateTicketDialogImpl = create<CreateTicketDialogProps>(
  ({ projectId, statusId }) => {
    const modal = useModal();
    const [title, setTitle] = useState('');
    const [description, setDescription] = useState('');
    const [externalId, setExternalId] = useState('');
    const [mode, setMode] = useState<IssueMode>('ask');
    const [saving, setSaving] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // A ticket that already has the External ID; creating again needs a
    // second click (D17).
    const [duplicate, setDuplicate] = useState<Issue | null>(null);

    // Fill the External ID from the text with the project's pattern (D16).
    const detectExternalId = () => {
      if (externalId.trim() || !(title.trim() || description.trim())) return;
      projectsApi
        .detectExternalId(projectId, `${title}\n${description}`)
        .then((id) => {
          if (id) setExternalId((current) => current || id);
        })
        .catch(() => {});
    };

    const close = (result: Issue | null) => {
      modal.resolve(result);
      modal.hide();
    };

    const submit = async () => {
      if (!title.trim()) {
        setError('A title is required.');
        return;
      }
      setSaving(true);
      setError(null);
      try {
        const trimmedId = externalId.trim();
        if (trimmedId && duplicate?.external_id !== trimmedId) {
          const existing = await projectsApi.findIssueByExternalId(
            projectId,
            trimmedId
          );
          if (existing) {
            setDuplicate(existing);
            return;
          }
        }
        const issue = await projectsApi.createIssue(projectId, {
          title,
          description: description.trim() || undefined,
          external_id: externalId.trim() || undefined,
          status_id: statusId,
          mode,
        });
        close(issue);
      } catch (err) {
        setError(getErrorMessage(err) || 'Failed to create ticket');
      } finally {
        setSaving(false);
      }
    };

    return (
      <Dialog
        open={modal.visible}
        onOpenChange={(open) => {
          if (!open) close(null);
        }}
      >
        <DialogContent className="sm:max-w-[560px]">
          <DialogHeader>
            <DialogTitle>New ticket</DialogTitle>
          </DialogHeader>
          <form
            className="flex flex-col gap-base py-base"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <div className="flex flex-col gap-half">
              <Label htmlFor="ticket-title">Title</Label>
              <Input
                id="ticket-title"
                autoFocus
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                onBlur={detectExternalId}
              />
            </div>
            <div className="flex flex-col gap-half">
              <Label htmlFor="ticket-description">Description</Label>
              <Textarea
                id="ticket-description"
                rows={6}
                placeholder="Markdown supported"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                onBlur={detectExternalId}
              />
            </div>
            <div className="grid grid-cols-2 gap-base">
              <div className="flex flex-col gap-half">
                <Label htmlFor="ticket-external-id">External ID</Label>
                <Input
                  id="ticket-external-id"
                  placeholder="e.g. PROJ-123"
                  value={externalId}
                  onChange={(e) => setExternalId(e.target.value)}
                />
              </div>
              <div className="flex flex-col gap-half">
                <Label htmlFor="ticket-mode">Agent mode</Label>
                <select
                  id="ticket-mode"
                  className="h-9 rounded-sm border bg-primary px-base text-sm"
                  value={mode}
                  onChange={(e) => setMode(e.target.value as IssueMode)}
                >
                  <option value="ask">Ask</option>
                  <option value="yolo">Yolo</option>
                </select>
              </div>
            </div>
            {duplicate && duplicate.external_id === externalId.trim() && (
              <p
                className="rounded-sm border border-brand/40 bg-brand/10 p-base text-sm"
                data-testid="duplicate-external-id"
              >
                A ticket with {duplicate.external_id} already exists: “
                {duplicate.title}”. Use Drop ticket to add a new version of it,
                or create another ticket anyway.
              </p>
            )}
            {error && <p className="text-sm text-error">{error}</p>}
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => close(null)}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={saving}>
                {duplicate && duplicate.external_id === externalId.trim()
                  ? 'Create anyway'
                  : 'Create ticket'}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    );
  }
);

export const CreateTicketDialog = defineModal<
  CreateTicketDialogProps,
  Issue | null
>(CreateTicketDialogImpl);

import { useCallback, useEffect, useState } from 'react';
import { Button } from '@kanbain/ui/components/Button';
import { Label } from '@kanbain/ui/components/Label';
import type {
  Project,
  ProjectSetupScan as SetupScan,
  SetupProposal,
  SetupScanResponse,
} from 'shared/types';
import { MarkdownPreview } from '@/shared/components/MarkdownPreview';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { getErrorMessage } from '@/shared/lib/modals';
import { getActualTheme } from '@/shared/lib/theme';
import { projectsApi } from '@/shared/lib/ticketsApi';

const POLL_MS = 3000;
const textareaClass =
  'rounded-sm border border-border bg-secondary px-base py-half font-mono text-sm';

interface ProjectSetupScanProps {
  projectId: string;
  /** The project after an approval, with the proposal in its settings. */
  onApproved: (project: Project) => void;
  onError: (message: string) => void;
}

const lines = (text: string) =>
  text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

/**
 * The setup agent's sandbox setup proposal: start a scan, review and edit
 * what it found, then approve it into the project settings or bin it.
 */
export function ProjectSetupScan({
  projectId,
  onApproved,
  onError,
}: ProjectSetupScanProps) {
  const [view, setView] = useState<SetupScanResponse | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    () =>
      projectsApi
        .setupScan(projectId)
        .then(setView)
        .catch((err: unknown) =>
          onError(getErrorMessage(err) || 'Failed to load the setup scan')
        ),
    [projectId, onError]
  );

  useEffect(() => {
    void load();
  }, [load]);

  const scan = view?.scan ?? null;
  const status = scan?.status;
  useEffect(() => {
    if (status !== 'scanning') return;
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [status, load]);

  const act = async (action: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await action();
      await load();
    } catch (err) {
      onError(getErrorMessage(err) || 'Setup scan action failed');
    } finally {
      setBusy(false);
    }
  };

  const approve = (proposal: SetupProposal) =>
    act(async () => {
      onApproved(await projectsApi.approveSetupScan(projectId, proposal));
    });

  if (!view) return null;
  const job = view.job;

  return (
    <div
      className="col-span-2 flex flex-col gap-base rounded-sm border border-border p-base"
      data-testid="project-setup-scan"
    >
      <div className="flex items-start justify-between gap-base">
        <div className="flex flex-col gap-half">
          <Label>Setup agent</Label>
          <p className="text-xs text-low">
            Reads the repos (and the web, where the README is vague) and
            proposes the setup script, test and run commands and notes every
            agent of this project gets. Nothing applies until you approve it.
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          disabled={busy || status === 'scanning'}
          onClick={() => void act(() => projectsApi.startSetupScan(projectId))}
        >
          {status === 'scanning'
            ? 'Scanning…'
            : scan
              ? 'Scan again'
              : 'Scan project'}
        </Button>
      </div>

      {view.stack_changed && status !== 'scanning' && status !== 'ready' && (
        <p className="text-sm text-warning">
          The repos’ stack changed since this setup was approved. The next agent
          run rescans it, or scan now.
        </p>
      )}

      {view.auto_steps.length > 0 && (
        <details className="text-xs text-low">
          <summary className="cursor-pointer">
            KanbAIn installs {view.auto_steps.length} thing
            {view.auto_steps.length === 1 ? '' : 's'} by itself
          </summary>
          <ul className="mt-half font-mono">
            {view.auto_steps.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ul>
        </details>
      )}

      {status === 'scanning' && (
        <p className="text-sm text-low">
          The setup agent is reading the repos. This takes a few minutes.
        </p>
      )}

      {status === 'failed' && (
        <p className="text-sm text-error">Scan failed: {scan?.error}</p>
      )}

      {status === 'approved' && scan?.decided_at && (
        <p className="text-xs text-low">
          Approved {new Date(scan.decided_at).toLocaleString()}; it is in the
          setup fields below.
        </p>
      )}

      {scan && status === 'ready' && (
        // Keyed by the proposal: a new scan resets the edits, a poll doesn't.
        <ProposalEditor
          key={scan.finished_at ?? scan.started_at}
          scan={scan}
          busy={busy}
          onApprove={(proposal) => void approve(proposal)}
          onDiscard={() =>
            void act(() => projectsApi.discardSetupScan(projectId))
          }
        />
      )}

      {job && (
        <p className="text-xs text-low">
          {job.agent_def_id} · {job.model} · ${job.cost_usd.toFixed(2)}
        </p>
      )}
    </div>
  );
}

interface ProposalEditorProps {
  scan: SetupScan;
  busy: boolean;
  onApprove: (proposal: SetupProposal) => void;
  onDiscard: () => void;
}

/** A ready proposal, editable before it is approved. */
function ProposalEditor({
  scan,
  busy,
  onApprove,
  onDiscard,
}: ProposalEditorProps) {
  const { config } = useUserSystem();
  const [script, setScript] = useState(scan.setup_script ?? '');
  const [tests, setTests] = useState(scan.test_commands.join('\n'));
  const [runs, setRuns] = useState(scan.run_commands.join('\n'));
  const [notes, setNotes] = useState(scan.environment_md ?? '');
  const [skipInstall, setSkipInstall] = useState(scan.skip_auto_install);

  return (
    <div className="flex flex-col gap-base">
      {scan.summary && (
        <MarkdownPreview
          content={scan.summary}
          theme={getActualTheme(config?.theme)}
          className="text-sm"
        />
      )}
      <div className="flex flex-col gap-half">
        <Label htmlFor="setup-scan-script">
          setup.sh (runs after KanbAIn’s installs)
        </Label>
        <textarea
          id="setup-scan-script"
          data-testid="setup-scan-script"
          className={`min-h-24 ${textareaClass}`}
          placeholder="Nothing extra needed"
          value={script}
          onChange={(e) => setScript(e.target.value)}
        />
      </div>
      <div className="grid grid-cols-2 gap-base">
        <div className="flex flex-col gap-half">
          <Label htmlFor="setup-scan-tests">Test commands</Label>
          <textarea
            id="setup-scan-tests"
            className={`min-h-16 ${textareaClass}`}
            value={tests}
            onChange={(e) => setTests(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-half">
          <Label htmlFor="setup-scan-runs">Build and run commands</Label>
          <textarea
            id="setup-scan-runs"
            className={`min-h-16 ${textareaClass}`}
            value={runs}
            onChange={(e) => setRuns(e.target.value)}
          />
        </div>
      </div>
      <div className="flex flex-col gap-half">
        <Label htmlFor="setup-scan-notes">environment.md notes</Label>
        <textarea
          id="setup-scan-notes"
          className={`min-h-24 ${textareaClass}`}
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
        />
      </div>
      <label className="flex items-center gap-base text-sm">
        <input
          type="checkbox"
          checked={skipInstall}
          onChange={(e) => setSkipInstall(e.target.checked)}
        />
        Skip KanbAIn’s own dependency installs (setup.sh does them)
      </label>
      <div className="flex justify-end gap-half">
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={onDiscard}
        >
          Discard
        </Button>
        <Button
          type="button"
          disabled={busy}
          data-testid="setup-scan-approve"
          onClick={() =>
            onApprove({
              setup_script: script,
              test_commands: lines(tests),
              run_commands: lines(runs),
              environment_md: notes,
              skip_auto_install: skipInstall,
            })
          }
        >
          Approve setup
        </Button>
      </div>
    </div>
  );
}

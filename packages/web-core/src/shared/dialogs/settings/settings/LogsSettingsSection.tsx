import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AppLogEntry } from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { logsApi } from '@/shared/lib/api';
import { SettingsCard, SettingsCheckbox } from './SettingsComponents';

const POLL_MS = 3000;
/** Lines kept on screen; the backend keeps 2000. */
const MAX_LINES = 2000;

function line(entry: AppLogEntry): string {
  return `${entry.timestamp} ${entry.level.padStart(5)} ${entry.target}: ${entry.message}`;
}

/**
 * The app's own warnings and errors, raw (Settings → Logs). The desktop app
 * has no console, so this is where a failed run, merge or index shows why.
 */
export function LogsSettingsSection() {
  const { t } = useTranslation('settings');
  const [entries, setEntries] = useState<AppLogEntry[]>([]);
  const [filePath, setFilePath] = useState<string | null>(null);
  const [warnings, setWarnings] = useState(true);
  const [follow, setFollow] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const lastSeq = useRef(0);
  const box = useRef<HTMLPreElement>(null);

  const poll = useCallback(async () => {
    try {
      const view = await logsApi.get({ since: lastSeq.current, warnings });
      setFilePath(view.file_path);
      setError(null);
      if (view.entries.length) {
        lastSeq.current = view.entries[view.entries.length - 1].seq;
        setEntries((current) =>
          [...current, ...view.entries].slice(-MAX_LINES)
        );
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [warnings]);

  // A different filter starts over.
  useEffect(() => {
    lastSeq.current = 0;
    setEntries([]);
    void poll();
    const timer = window.setInterval(() => void poll(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [poll]);

  useEffect(() => {
    if (follow && box.current) {
      box.current.scrollTop = box.current.scrollHeight;
    }
  }, [entries, follow]);

  const copy = async () => {
    await navigator.clipboard.writeText(entries.map(line).join('\n'));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const clear = async () => {
    try {
      await logsApi.clear();
      setEntries([]);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="space-y-6">
      <SettingsCard
        title={t('settings.logs.title')}
        description={t('settings.logs.description')}
        headerAction={
          <div className="flex gap-2">
            <PrimaryButton
              variant="secondary"
              value={
                copied ? t('settings.logs.copied') : t('settings.logs.copy')
              }
              onClick={() => void copy()}
              disabled={!entries.length}
            />
            <PrimaryButton
              variant="tertiary"
              value={t('settings.logs.clear')}
              onClick={() => void clear()}
              disabled={!entries.length}
            />
          </div>
        }
      >
        <div className="flex flex-wrap gap-6">
          <SettingsCheckbox
            id="logs-warnings"
            label={t('settings.logs.warnings')}
            checked={warnings}
            onChange={setWarnings}
          />
          <SettingsCheckbox
            id="logs-follow"
            label={t('settings.logs.follow')}
            checked={follow}
            onChange={setFollow}
          />
        </div>
        {error && <p className="text-sm text-error">{error}</p>}
        <pre
          ref={box}
          data-testid="app-log"
          className="h-[28rem] overflow-auto whitespace-pre-wrap break-all rounded-sm border bg-secondary p-base font-mono text-xs text-normal"
        >
          {entries.length
            ? entries.map((entry) => (
                <div
                  key={entry.seq}
                  className={
                    entry.level === 'ERROR' ? 'text-error' : 'text-normal'
                  }
                >
                  {line(entry)}
                </div>
              ))
            : t('settings.logs.empty')}
        </pre>
        {filePath && (
          <p className="text-xs text-low">
            {t('settings.logs.file')}{' '}
            <span className="font-mono" data-testid="app-log-file">
              {filePath}
            </span>
          </p>
        )}
      </SettingsCard>
    </div>
  );
}

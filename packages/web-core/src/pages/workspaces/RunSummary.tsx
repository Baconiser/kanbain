import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ShieldCheckIcon, ProhibitIcon } from '@phosphor-icons/react';
import type { ExecutionProcess } from 'shared/types';
import { executionProcessesApi } from '@/shared/lib/api';

function formatUsd(value: number): string {
  return `$${value < 0.01 && value > 0 ? value.toFixed(4) : value.toFixed(2)}`;
}

function formatTokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

const EXTEND_MINUTES = 30;

/** When a running run hits its time limit, with a button to extend it. */
function TimeLimit({ processId }: { processId: string }) {
  const { t } = useTranslation('common');
  const queryClient = useQueryClient();
  const key = ['execution-process-time-limit', processId];
  const { data } = useQuery({
    queryKey: key,
    queryFn: () => executionProcessesApi.getTimeLimit(processId),
    refetchInterval: 30_000,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!data?.deadline) return null;
  const deadline = new Date(data.deadline);
  const minutesLeft = Math.max(
    0,
    Math.ceil((deadline.getTime() - Date.now()) / 60_000)
  );

  const extend = async () => {
    setBusy(true);
    setError(null);
    try {
      queryClient.setQueryData(
        key,
        await executionProcessesApi.extendTimeLimit(processId, EXTEND_MINUTES)
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <span className="text-low">{t('runSummary.timeLimit')}</span>
      <span className="flex flex-wrap items-center gap-half">
        <span className={minutesLeft <= 5 ? 'text-error' : undefined}>
          {t('runSummary.timeLeft', {
            time: deadline.toLocaleTimeString([], {
              hour: '2-digit',
              minute: '2-digit',
            }),
            count: minutesLeft,
          })}
        </span>
        <button
          type="button"
          className="text-brand underline disabled:opacity-50"
          onClick={extend}
          disabled={busy}
        >
          {t('runSummary.extend', { count: EXTEND_MINUTES })}
        </button>
        {error && <span className="text-error">{error}</span>}
      </span>
    </>
  );
}

/** Sandbox details of one run: model usage, cost against budget, blocks. */
export function RunSummary({ process }: { process: ExecutionProcess }) {
  const { t } = useTranslation('common');
  const running = process.status === 'running';
  const { data: events } = useQuery({
    queryKey: ['execution-process-events', process.id],
    queryFn: () => executionProcessesApi.getEvents(process.id),
    refetchInterval: running ? 3_000 : false,
  });

  if (!process.container_id) return null;

  const modelCalls =
    events?.filter((e) => e.event_type === 'model_call').length ?? 0;
  const blocked = (events ?? []).filter((e) => e.event_type === 'egress_block');
  const blockedHosts = [
    ...new Set(blocked.map((e) => String(e.payload.host ?? ''))),
  ].filter(Boolean);
  const budgetStop = events?.some((e) => e.event_type === 'budget_exceeded');

  return (
    <div className="mx-base mb-base rounded-sm border border-border p-half text-xs text-normal space-y-half">
      <div className="flex items-center gap-half text-low">
        <ShieldCheckIcon className="size-icon-sm" />
        <span>{t('runSummary.sandboxed')}</span>
      </div>
      <div className="grid grid-cols-2 gap-x-base gap-y-0.5 tabular-nums">
        <span className="text-low">{t('runSummary.cost')}</span>
        <span>
          {formatUsd(process.cost_usd)}
          {process.budget_usd != null && ` / ${formatUsd(process.budget_usd)}`}
        </span>
        <span className="text-low">{t('runSummary.tokens')}</span>
        <span>
          {formatTokens(process.tokens_in)} → {formatTokens(process.tokens_out)}
        </span>
        <span className="text-low">{t('runSummary.cache')}</span>
        <span>
          {formatTokens(process.tokens_cache_read)} /{' '}
          {formatTokens(process.tokens_cache_write)}
        </span>
        <span className="text-low">{t('runSummary.modelCalls')}</span>
        <span>{modelCalls}</span>
        {running && <TimeLimit processId={process.id} />}
      </div>
      {budgetStop && <p className="text-error">{t('runSummary.budgetStop')}</p>}
      {blockedHosts.length > 0 && (
        <div className="flex items-start gap-half">
          <ProhibitIcon className="size-icon-sm text-low shrink-0 mt-px" />
          <span>
            {t('runSummary.blocked', { hosts: blockedHosts.join(', ') })}
          </span>
        </div>
      )}
    </div>
  );
}

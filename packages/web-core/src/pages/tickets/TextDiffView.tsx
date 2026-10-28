import { useEffect, useState } from 'react';
import type { TextDiffLine } from 'shared/types';
import { textDiff } from '@/shared/lib/ticketsApi';

interface TextDiffViewProps {
  oldText: string;
  newText: string;
}

const LINE_STYLES: Record<TextDiffLine['kind'], string> = {
  equal: 'text-low',
  insert: 'bg-success/15 text-normal',
  delete: 'bg-error/15 text-normal line-through decoration-error/60',
};

const LINE_MARKS: Record<TextDiffLine['kind'], string> = {
  equal: ' ',
  insert: '+',
  delete: '-',
};

/** Line diff of two texts, computed by the backend. */
export function TextDiffView({ oldText, newText }: TextDiffViewProps) {
  const [lines, setLines] = useState<TextDiffLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    textDiff(oldText, newText)
      .then((result) => {
        if (!cancelled) setLines(result);
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(err instanceof Error ? err.message : 'Diff failed');
      });
    return () => {
      cancelled = true;
    };
  }, [oldText, newText]);

  if (error) return <p className="text-sm text-error">{error}</p>;
  if (!lines) return <p className="text-sm text-low">Comparing…</p>;
  if (lines.every((l) => l.kind === 'equal')) {
    return <p className="text-sm text-low">No changes in the description.</p>;
  }

  return (
    <div
      className="max-h-72 overflow-auto rounded-sm border bg-primary font-ibm-plex-mono text-xs"
      data-testid="text-diff"
    >
      {lines.map((line, i) => (
        <div
          key={i}
          className={`whitespace-pre-wrap px-half ${LINE_STYLES[line.kind]}`}
          data-kind={line.kind}
        >
          <span className="mr-half select-none text-low">
            {LINE_MARKS[line.kind]}
          </span>
          {line.text || ' '}
        </div>
      ))}
    </div>
  );
}

import { useEffect, useRef } from 'react';
import { useAppStore } from '../../stores/appStore';
import type { NodeConsoleEntry } from '../../api/types';

const EMPTY_ENTRIES: NodeConsoleEntry[] = [];
const LEVEL_CLASS: Record<string, string> = {
  ERROR: 'log-level-error',
  WARN: 'log-level-warn',
  INFO: 'log-level-info',
  DEBUG: 'log-level-debug',
  TRACE: 'log-level-debug',
};

interface ConsolePanelProps {
  nodeKey: string;
}

export default function ConsolePanel({ nodeKey }: ConsolePanelProps) {
  const entries = useAppStore((s) => s.nodeConsoleLogs[nodeKey] ?? EMPTY_ENTRIES);
  const clearNodeConsoleLogs = useAppStore((s) => s.clearNodeConsoleLogs);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [entries]);

  return (
    <div className="console-panel">
      <div className="console-header">
        <span className="console-node-key">{nodeKey}</span>
        <span className="console-count">{entries.length} entries</span>
        <button className="secondary console-clear" onClick={() => clearNodeConsoleLogs(nodeKey)}>
          Clear
        </button>
      </div>
      <div className="console-entries" ref={containerRef}>
        {entries.length === 0 && (
          <div className="console-empty">No log output yet</div>
        )}
        {entries.map((entry, i) => (
          <div key={i} className={`console-line ${LEVEL_CLASS[entry.level] || ''}`}>
            <span className="console-ts">{entry.ts}</span>
            <span className={`console-level ${LEVEL_CLASS[entry.level] || ''}`}>
              {entry.level.padEnd(5)}
            </span>
            <span className="console-msg">{entry.message}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

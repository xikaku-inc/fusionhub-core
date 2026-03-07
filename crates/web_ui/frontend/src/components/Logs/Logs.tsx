import { useEffect, useRef, useState, useMemo } from 'react';
import { useAppStore } from '../../stores/appStore';
import { apiGet } from '../../api/client';
import type { LogEntry } from '../../api/types';

const LEVEL_CLASS: Record<string, string> = {
  ERROR: 'log-level-error',
  WARN: 'log-level-warn',
  INFO: 'log-level-info',
  DEBUG: 'log-level-debug',
  TRACE: 'log-level-debug',
};

const LEVELS = ['TRACE', 'DEBUG', 'INFO', 'WARN', 'ERROR'];

export default function Logs() {
  const logEntries = useAppStore((s) => s.logEntries);
  const clearLogEntries = useAppStore((s) => s.clearLogEntries);
  const [autoScroll, setAutoScroll] = useState(true);
  const [minLevel, setMinLevel] = useState('TRACE');
  const containerRef = useRef<HTMLDivElement>(null);
  const fetchedRef = useRef(false);

  // Fetch buffered log history on first mount
  useEffect(() => {
    if (fetchedRef.current) return;
    fetchedRef.current = true;
    apiGet<LogEntry[]>('/api/logs').then((entries) => {
      if (entries.length > 0) {
        useAppStore.getState().addLogEntries(entries);
      }
    }).catch(() => {});
  }, []);

  const minLevelIdx = LEVELS.indexOf(minLevel);
  const filtered = useMemo(
    () => logEntries.filter((e) => LEVELS.indexOf(e.level) >= minLevelIdx),
    [logEntries, minLevelIdx],
  );

  useEffect(() => {
    if (autoScroll && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [filtered, autoScroll]);

  function handleScroll() {
    const el = containerRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setAutoScroll(atBottom);
  }

  return (
    <div className="view-container">
      <div className="log-toolbar">
        <div className="flex-row gap-8">
          <div className="select-wrapper">
            <select value={minLevel} onChange={(e) => setMinLevel(e.target.value)}>
              {LEVELS.map((l) => (
                <option key={l} value={l}>{l}</option>
              ))}
            </select>
          </div>
          <button className="secondary" onClick={clearLogEntries}>Clear</button>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
            />
            Auto-scroll
          </label>
        </div>
        <span style={{ color: 'var(--text2)', fontSize: 12 }}>
          {filtered.length} entries
        </span>
      </div>
      <div
        ref={containerRef}
        className="log-viewer"
        onScroll={handleScroll}
      >
        {filtered.map((entry, i) => (
          <div key={i} className={`log-line ${LEVEL_CLASS[entry.level] || ''}`}>
            <span className="log-line-ts">{entry.ts}</span>
            <span className={`log-line-level ${LEVEL_CLASS[entry.level] || ''}`}>
              {entry.level.padEnd(5)}
            </span>
            <span className="log-line-target">{entry.target}</span>
            <span className="log-line-msg">{entry.message}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

import { useAppStore } from '../../stores/appStore';
import type { NodeStatusEntry, NodeRateEntry } from '../../api/types';

function formatValue(v: any): string {
  if (v === null || v === undefined) return '-';
  if (typeof v === 'boolean') return v ? 'Yes' : 'No';
  if (typeof v === 'number') return Number.isInteger(v) ? v.toString() : v.toFixed(4);
  return String(v);
}

function StatusTile({ configKey, entry, rates }: {
  configKey: string;
  entry: NodeStatusEntry;
  rates: NodeRateEntry | undefined;
}) {
  const inputRate = rates?.inputRate ?? 0;
  const outputRate = rates?.outputRate ?? 0;
  const active = inputRate > 0 || outputRate > 0;

  const details = entry.nodeStatus && typeof entry.nodeStatus === 'object'
    ? Object.entries(entry.nodeStatus)
    : [];

  return (
    <div className="status-tile">
      <div className="tile-accent" style={{ background: entry.color }} />
      <div className="tile-header">
        <span className={`activity-dot ${active ? 'active' : 'inactive'}`} />
        <span className="tile-name" title={configKey}>{entry.displayName}</span>
        <span className="tile-role">{entry.role}</span>
      </div>
      <div className="tile-counters">
        <div className="tile-counter">
          <span className="tile-counter-label">In</span>
          <span className="tile-counter-value">{entry.inputCount}</span>
          <span className="tile-counter-rate">{inputRate > 0 ? inputRate + '/s' : '-'}</span>
        </div>
        <div className="tile-counter">
          <span className="tile-counter-label">Out</span>
          <span className="tile-counter-value">{entry.outputCount}</span>
          <span className="tile-counter-rate">{outputRate > 0 ? outputRate + '/s' : '-'}</span>
        </div>
      </div>
      {details.length > 0 && (
        <div className="tile-details">
          {details.map(([k, v]) => (
            <div key={k} className="tile-detail-row">
              <span className="tile-detail-key">{k}</span>
              <span className="tile-detail-value">{formatValue(v)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default function Dashboard() {
  const nodeStatuses = useAppStore((s) => s.nodeStatuses);
  const nodeRates = useAppStore((s) => s.nodeRates);
  const paused = useAppStore((s) => s.paused);
  const togglePause = useAppStore((s) => s.togglePause);
  const restart = useAppStore((s) => s.restart);

  const entries = Object.entries(nodeStatuses);
  const sources = entries.filter(([, e]) => e.role === 'source');
  const filters = entries.filter(([, e]) => e.role === 'filter');
  const sinks = entries.filter(([, e]) => e.role === 'sink');

  const sections = [
    { label: 'Sources', items: sources },
    { label: 'Filters', items: filters },
    { label: 'Sinks', items: sinks },
  ].filter((s) => s.items.length > 0);

  if (entries.length === 0) {
    return (
      <div className="view-container">
        <div className="card">
          <div className="card-body" style={{ color: 'var(--text2)' }}>
            Waiting for node status data...
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="view-container">
      <div className="dashboard-controls">
        <button
          className={paused ? 'secondary' : ''}
          onClick={togglePause}
          title={paused ? 'Resume data flow' : 'Pause data flow'}
        >
          {paused ? '\u25B6 Resume' : '\u275A\u275A Pause'}
        </button>
        <button className="danger" onClick={restart} title="Restart node graph">
          {'\u21BB'} Reset
        </button>
      </div>
      {sections.map((sec) => (
        <div key={sec.label}>
          <div className="tile-section-title">{sec.label}</div>
          <div className="tile-grid">
            {sec.items.map(([key, entry]) => (
              <StatusTile
                key={key}
                configKey={key}
                entry={entry}
                rates={nodeRates[key]}
              />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

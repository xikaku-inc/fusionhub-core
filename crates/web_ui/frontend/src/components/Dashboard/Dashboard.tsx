import { useAppStore } from '../../stores/appStore';
import type { NodeStatusEntry, NodeRateEntry, McpStatus } from '../../api/types';

function formatValue(v: any): string {
  if (v === null || v === undefined) return '-';
  if (typeof v === 'boolean') return v ? 'Yes' : 'No';
  if (typeof v === 'number') return Number.isInteger(v) ? v.toString() : v.toFixed(4);
  return String(v);
}

function formatRelativeTime(isoString: string): string {
  const diff = (Date.now() - new Date(isoString).getTime()) / 1000;
  if (diff < 60) return `${Math.floor(diff)}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ${Math.floor(diff % 60)}s ago`;
  return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m ago`;
}

function formatTime(isoString: string): string {
  try {
    return new Date(isoString).toLocaleTimeString();
  } catch {
    return isoString;
  }
}

function McpStatusCard({ status }: { status: McpStatus }) {
  return (
    <div className="mcp-status-card">
      <div className="mcp-accent" />
      <div className="mcp-header">
        <span className={`activity-dot ${status.connected ? 'active' : 'inactive'}`} />
        <span className="mcp-title">MCP Client</span>
        <span className={`mcp-conn-badge ${status.connected ? 'connected' : 'disconnected'}`}>
          {status.connected ? 'Connected' : 'Disconnected'}
        </span>
      </div>
      <div className="mcp-info">
        {status.clientName && (
          <div className="mcp-info-row">
            <span className="mcp-info-label">Client</span>
            <span className="mcp-info-value">{status.clientName}</span>
          </div>
        )}
        {status.connectedSince && (
          <div className="mcp-info-row">
            <span className="mcp-info-label">Uptime</span>
            <span className="mcp-info-value">{formatRelativeTime(status.connectedSince)}</span>
          </div>
        )}
        <div className="mcp-info-row">
          <span className="mcp-info-label">Tool calls</span>
          <span className="mcp-info-value">{status.toolCallCount}</span>
        </div>
        {status.lastTool && status.lastToolTime && (
          <div className="mcp-info-row">
            <span className="mcp-info-label">Last</span>
            <span className="mcp-info-value">
              {status.lastTool} ({formatRelativeTime(status.lastToolTime)})
            </span>
          </div>
        )}
      </div>
      {status.recentTools.length > 0 && (
        <div className="mcp-tool-list">
          <div className="mcp-tool-list-title">Recent</div>
          {status.recentTools.slice().reverse().map((entry, i) => (
            <div key={i} className="mcp-tool-entry">
              <span className="mcp-tool-name">{entry.tool}</span>
              <span className="mcp-tool-time">{formatTime(entry.time)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
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
  const mcpStatus = useAppStore((s) => s.mcpStatus);

  const entries = Object.entries(nodeStatuses);
  const sources = entries.filter(([, e]) => e.role === 'source');
  const filters = entries.filter(([, e]) => e.role === 'filter');
  const sinks = entries.filter(([, e]) => e.role === 'sink');

  const sections = [
    { label: 'Sources', items: sources },
    { label: 'Filters', items: filters },
    { label: 'Sinks', items: sinks },
  ].filter((s) => s.items.length > 0);

  if (entries.length === 0 && !mcpStatus) {
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
      {mcpStatus && <McpStatusCard status={mcpStatus} />}
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

import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';
import { useAppStore } from '../../../stores/appStore';
import { getNodeRate, statusColor } from '../utils/nodeStatusMapper';

export default function SourceNode({ data, selected }: NodeProps) {
  const d = data as EditorNode;
  const inputRates = useAppStore((s) => s.inputRates);
  const fusionRates = useAppStore((s) => s.fusionRates);
  const rate = getNodeRate(d.nodeType.id, d.configKey, 'source', inputRates, fusionRates);
  const active = d.active as boolean | undefined;
  const status = active ? 'active' : rate > 0 ? 'active' : 'idle';

  return (
    <div className={`fh-node ${selected ? 'selected' : ''} ${d.disabled ? 'disabled' : ''}`}
         style={{ borderColor: d.nodeType.color }}>
      <div className="node-header" style={{ background: d.nodeType.color, color: '#000' }}>
        <span className="node-status-dot" style={{ background: statusColor(status) }} />
        <span className="node-role-badge">SRC</span>
        <span className="node-label">{d.configKey}</span>
        {d.subtype && <span className="node-subtype">{d.subtype}</span>}
        {rate > 0 && <span className="node-rate">{rate} Hz</span>}
      </div>
      <div className="node-body">
        {d.nodeType.outputs.map((dt) => (
          <div key={dt} className="node-port output-port">
            <span className="port-label">{dt}</span>
            <Handle
              type="source"
              position={Position.Right}
              id={`out-${dt}`}
              className="port-handle source-handle"
            />
          </div>
        ))}
      </div>
    </div>
  );
}

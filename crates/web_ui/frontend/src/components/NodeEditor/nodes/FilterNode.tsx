import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';
import { statusColor } from '../utils/nodeStatusMapper';

export default function FilterNode({ data, selected }: NodeProps) {
  const d = data as EditorNode;
  const active = d.active as boolean | undefined;
  const status = active ? 'active' : 'idle';

  return (
    <div className={`fh-node ${selected ? 'selected' : ''} ${d.disabled ? 'disabled' : ''}`}
         style={{ borderColor: d.nodeType.color }}>
      <div className="node-header" style={{ background: d.nodeType.color, color: '#fff' }}>
        <span className="node-status-dot" style={{ background: statusColor(status) }} />
        <span className="node-role-badge">FLT</span>
        <span className="node-label">{d.configKey}</span>
      </div>
      <div className="node-body">
        <div className="node-ports-row">
          <div className="node-ports-col">
            {d.nodeType.inputs.map((dt) => (
              <div key={`in-${dt}`} className="node-port input-port">
                <Handle
                  type="target"
                  position={Position.Left}
                  id={`in-${dt}`}
                  className="port-handle target-handle"
                />
                <span className="port-label left">{dt}</span>
              </div>
            ))}
          </div>
          <div className="node-ports-col">
            {d.nodeType.outputs.map((dt) => (
              <div key={`out-${dt}`} className="node-port output-port">
                <span className="port-label right">{dt}</span>
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
      </div>
    </div>
  );
}

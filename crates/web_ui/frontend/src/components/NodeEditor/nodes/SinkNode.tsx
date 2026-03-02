import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';
import { statusColor } from '../utils/nodeStatusMapper';

export default function SinkNode({ data, selected }: NodeProps) {
  const d = data as EditorNode;
  const hasSpecificInputs = d.nodeType.inputs.length > 0;

  return (
    <div className={`fh-node ${selected ? 'selected' : ''} ${d.disabled ? 'disabled' : ''}`}
         style={{ borderColor: d.nodeType.color }}>
      <div className="node-header" style={{ background: d.nodeType.color, color: '#fff' }}>
        <span className="node-status-dot" style={{ background: statusColor((d.active as boolean) ? 'active' : 'inactive') }} />
        <span className="node-role-badge">OUT</span>
        <span className="node-label">{d.configKey}</span>
      </div>
      <div className="node-body">
        {hasSpecificInputs ? (
          d.nodeType.inputs.map((dt) => (
            <div key={`in-${dt}`} className="node-port input-port">
              <Handle
                type="target"
                position={Position.Left}
                id={`in-${dt}`}
                className="port-handle target-handle"
              />
              <span className="port-label left">{dt}</span>
            </div>
          ))
        ) : (
          <div className="node-port input-port">
            <Handle
              type="target"
              position={Position.Left}
              id="in-any"
              className="port-handle target-handle"
            />
            <span className="port-label left">Any</span>
          </div>
        )}
      </div>
    </div>
  );
}

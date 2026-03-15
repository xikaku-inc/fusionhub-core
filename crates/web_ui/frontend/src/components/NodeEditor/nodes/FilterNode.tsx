import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';
import { statusColor } from '../utils/nodeStatusMapper';

export default function FilterNode({ data, selected }: NodeProps) {
  const d = data as EditorNode;
  const active = d.active as boolean | undefined;
  const status = active ? 'active' : 'idle';

  const hasSpecificInputs = d.nodeType.inputs.length > 0;
  const hasSpecificOutputs = d.nodeType.outputs.length > 0;
  const inputCount = hasSpecificInputs ? 0 : Math.max(1, Number(d.settings?.inputCount) || 1);
  const outputCount = hasSpecificOutputs ? 0 : Math.max(1, Number(d.settings?.outputCount) || 1);

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
              Array.from({ length: inputCount }, (_, i) => (
                <div key={`in-any-${i}`} className="node-port input-port">
                  <Handle
                    type="target"
                    position={Position.Left}
                    id={`in-any-${i}`}
                    className="port-handle target-handle"
                  />
                  <span className="port-label left">{inputCount === 1 ? 'Any' : `In ${i + 1}`}</span>
                </div>
              ))
            )}
          </div>
          <div className="node-ports-col">
            {hasSpecificOutputs ? (
              d.nodeType.outputs.map((dt) => (
                <div key={`out-${dt}`} className="node-port output-port">
                  <span className="port-label right">{dt}</span>
                  <Handle
                    type="source"
                    position={Position.Right}
                    id={`out-${dt}`}
                    className="port-handle source-handle"
                  />
                </div>
              ))
            ) : (
              Array.from({ length: outputCount }, (_, i) => (
                <div key={`out-any-${i}`} className="node-port output-port">
                  <span className="port-label right">{outputCount === 1 ? 'Any' : `Out ${i + 1}`}</span>
                  <Handle
                    type="source"
                    position={Position.Right}
                    id={`out-any-${i}`}
                    className="port-handle source-handle"
                  />
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

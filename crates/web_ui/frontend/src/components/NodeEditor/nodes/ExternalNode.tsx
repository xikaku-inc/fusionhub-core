import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';

export default function ExternalNode({ data, selected }: NodeProps) {
  const d = data as EditorNode;
  const isInput = d.externalDirection === 'input';
  const color = d.nodeType.color;

  const label = d.endpoint
    .replace('tcp://*:', ':')
    .replace('tcp://0.0.0.0:', ':')
    .replace('tcp://', '');

  return (
    <div
      className={`fh-node fh-external-node ${selected ? 'selected' : ''}`}
      style={{ borderColor: color }}
    >
      <div className="node-header external-header" style={{ background: color, color: '#000' }}>
        <span className="node-role-badge">{isInput ? 'IN' : 'OUT'}</span>
        <span className="node-label">{label}</span>
      </div>
      {isInput && (
        <Handle
          type="source"
          position={Position.Right}
          id="out-ext"
          className="port-handle source-handle"
        />
      )}
      {!isInput && (
        <Handle
          type="target"
          position={Position.Left}
          id="in-ext"
          className="port-handle target-handle"
        />
      )}
    </div>
  );
}

import { type DragEvent } from 'react';
import { useAppStore } from '../../stores/appStore';
import type { NodeTypeDefinition, NodeRole } from '../../types/nodes';

const roles: { role: NodeRole; label: string }[] = [
  { role: 'source', label: 'Sources' },
  { role: 'filter', label: 'Filters' },
  { role: 'sink', label: 'Sinks' },
];

function onDragStart(event: DragEvent, nodeType: NodeTypeDefinition) {
  event.dataTransfer.setData('application/fusionhub-node', JSON.stringify(nodeType));
  event.dataTransfer.effectAllowed = 'move';
}

export default function NodePalette() {
  const nodeTypes = useAppStore((s) => s.nodeTypes);
  return (
    <div className="node-palette">
      <div className="palette-title">Node Library</div>
      {roles.map(({ role, label }) => {
        const types = nodeTypes.filter((n) => n.role === role);
        return (
          <div key={role} className="palette-section">
            <div className="palette-section-title">{label}</div>
            {types.map((nt) => (
              <div
                key={nt.id}
                className="palette-item"
                style={{ borderLeftColor: nt.color }}
                draggable
                onDragStart={(e) => onDragStart(e, nt)}
              >
                <span className="palette-item-name">{nt.displayName}</span>
                <span className="palette-item-io">
                  {nt.inputs.length > 0 && <span className="palette-inputs">{nt.inputs.join(', ')}</span>}
                  {nt.inputs.length > 0 && nt.outputs.length > 0 && ' → '}
                  {nt.outputs.length > 0 && <span className="palette-outputs">{nt.outputs.join(', ')}</span>}
                </span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}

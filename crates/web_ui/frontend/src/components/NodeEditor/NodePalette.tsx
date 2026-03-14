import { type DragEvent } from 'react';
import { useAppStore } from '../../stores/appStore';
import type { NodeTypeDefinition, NodeRole } from '../../types/nodes';
import { EXTERNAL_INPUT_TYPE, EXTERNAL_OUTPUT_TYPE } from './utils/externalNodeTypes';

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
                  {nt.role === 'sink' && nt.inputs.length === 0 ? (
                    <span className="palette-inputs">Any</span>
                  ) : (
                    <>
                      {nt.inputs.length > 0 && <span className="palette-inputs">{nt.inputs.join(', ')}</span>}
                      {nt.inputs.length > 0 && nt.outputs.length > 0 && ' → '}
                      {nt.outputs.length > 0 && <span className="palette-outputs">{nt.outputs.join(', ')}</span>}
                    </>
                  )}
                </span>
              </div>
            ))}
          </div>
        );
      })}
      <div className="palette-section">
        <div className="palette-section-title">External</div>
        <div
          className="palette-item"
          style={{ borderLeftColor: EXTERNAL_INPUT_TYPE.color }}
          draggable
          onDragStart={(e) => onDragStart(e, EXTERNAL_INPUT_TYPE)}
        >
          <span className="palette-item-name">TCP Input</span>
          <span className="palette-item-io">
            <span className="palette-outputs">ext</span>
          </span>
        </div>
        <div
          className="palette-item"
          style={{ borderLeftColor: EXTERNAL_OUTPUT_TYPE.color }}
          draggable
          onDragStart={(e) => onDragStart(e, EXTERNAL_OUTPUT_TYPE)}
        >
          <span className="palette-item-name">TCP Output</span>
          <span className="palette-item-io">
            <span className="palette-inputs">ext</span>
          </span>
        </div>
      </div>
    </div>
  );
}

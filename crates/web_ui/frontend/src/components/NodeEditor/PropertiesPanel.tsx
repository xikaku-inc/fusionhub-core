import { type Node } from '@xyflow/react';
import type { EditorNode, SettingsField } from '../../types/nodes';

interface Props {
  node: Node | null;
  onUpdateNode: (id: string, data: Partial<EditorNode>) => void;
}

function SettingsFieldInput({ field, value, onChange }: { field: SettingsField; value: any; onChange: (v: any) => void }) {
  switch (field.type) {
    case 'string':
      return <input type="text" value={value ?? field.default ?? ''} onChange={(e) => onChange(e.target.value)} />;
    case 'number':
      return <input type="text" value={value ?? field.default ?? ''} onChange={(e) => onChange(Number(e.target.value) || 0)} />;
    case 'boolean':
      return (
        <label className="checkbox">
          <input type="checkbox" checked={value ?? field.default ?? false} onChange={(e) => onChange(e.target.checked)} />
          {field.label}
        </label>
      );
    case 'quaternion': {
      const q = value ?? field.default ?? { w: 1, x: 0, y: 0, z: 0 };
      return (
        <div className="flex-col gap-4">
          {['w', 'x', 'y', 'z'].map((k) => (
            <div key={k} className="flex-row gap-4">
              <span style={{ width: 16, color: 'var(--text2)', fontSize: 12 }}>{k}:</span>
              <input type="text" value={q[k] ?? 0}
                onChange={(e) => onChange({ ...q, [k]: Number(e.target.value) || 0 })}
                style={{ flex: 1 }} />
            </div>
          ))}
        </div>
      );
    }
    case 'vector3': {
      const v = value ?? field.default ?? { x: 0, y: 0, z: 0 };
      return (
        <div className="flex-col gap-4">
          {['x', 'y', 'z'].map((k) => (
            <div key={k} className="flex-row gap-4">
              <span style={{ width: 16, color: 'var(--text2)', fontSize: 12 }}>{k}:</span>
              <input type="text" value={v[k] ?? 0}
                onChange={(e) => onChange({ ...v, [k]: Number(e.target.value) || 0 })}
                style={{ flex: 1 }} />
            </div>
          ))}
        </div>
      );
    }
    case 'json': {
      const str = typeof value === 'string' ? value : JSON.stringify(value ?? field.default ?? {}, null, 2);
      return (
        <textarea
          style={{ fontFamily: 'monospace', fontSize: 12, minHeight: 80, resize: 'vertical' }}
          value={str}
          onChange={(e) => {
            try { onChange(JSON.parse(e.target.value)); } catch { /* keep raw string */ }
          }}
        />
      );
    }
    default:
      return <input type="text" value={String(value ?? '')} onChange={(e) => onChange(e.target.value)} />;
  }
}

export default function PropertiesPanel({ node, onUpdateNode }: Props) {
  if (!node) {
    return (
      <div className="properties-panel">
        <div className="properties-title">Properties</div>
        <div className="properties-empty">Select a node to view its properties</div>
      </div>
    );
  }

  const d = node.data as EditorNode;

  if (d.externalDirection) {
    return (
      <div className="properties-panel">
        <div className="properties-title">Properties</div>
        <div className="properties-content">
          <div className="prop-section">
            <div className="prop-label">Type</div>
            <div className="prop-value">
              {d.externalDirection === 'input' ? 'External Input' : 'External Output'}
            </div>
          </div>
          <div className="prop-section">
            <div className="prop-label">TCP Endpoint</div>
            <input
              type="text"
              value={d.endpoint}
              onChange={(e) => onUpdateNode(node.id, { endpoint: e.target.value })}
            />
          </div>
        </div>
      </div>
    );
  }

  const { nodeType } = d;

  const updateSetting = (key: string, value: any) => {
    const newSettings = { ...d.settings };
    // Handle nested keys like "SensorFusion.alignment"
    const parts = key.split('.');
    if (parts.length === 1) {
      newSettings[key] = value;
    } else {
      let obj = newSettings;
      for (let i = 0; i < parts.length - 1; i++) {
        if (!obj[parts[i]]) obj[parts[i]] = {};
        obj = obj[parts[i]];
      }
      obj[parts[parts.length - 1]] = value;
    }
    onUpdateNode(node.id, { settings: newSettings });
  };

  const getSettingValue = (key: string): any => {
    const parts = key.split('.');
    let obj: any = d.settings;
    for (const part of parts) {
      if (obj == null) return undefined;
      obj = obj[part];
    }
    return obj;
  };

  // Get all settings fields including subtype-specific ones
  const allFields = [...nodeType.settingsSchema];
  if (nodeType.subtypes && d.subtype) {
    const sub = nodeType.subtypes.find((s) => s.value === d.subtype);
    if (sub) allFields.push(...sub.additionalSettings);
  }

  return (
    <div className="properties-panel">
      <div className="properties-title">Properties</div>
      <div className="properties-content">
        <div className="prop-section">
          <div className="prop-label">Name</div>
          <input
            type="text"
            value={d.configKey}
            onChange={(e) => onUpdateNode(node.id, { configKey: e.target.value })}
          />
        </div>

        <div className="prop-section">
          <div className="prop-label">Type</div>
          <div className="prop-value">{nodeType.displayName}</div>
        </div>

        {nodeType.subtypes && nodeType.subtypes.length > 0 && (
          <div className="prop-section">
            <div className="prop-label">Subtype</div>
            <input
              type="text"
              value={d.subtype || ''}
              onChange={(e) => onUpdateNode(node.id, { subtype: e.target.value })}
            />
          </div>
        )}

        <div className="prop-section">
          <div className="prop-label">Endpoint</div>
          <input
            type="text"
            value={d.endpoint}
            onChange={(e) => onUpdateNode(node.id, { endpoint: e.target.value })}
          />
        </div>

        <div className="prop-section">
          <label className="checkbox">
            <input
              type="checkbox"
              checked={!d.disabled}
              onChange={(e) => onUpdateNode(node.id, { disabled: !e.target.checked })}
            />
            Enabled
          </label>
        </div>

        {allFields.length > 0 && (
          <>
            <div className="prop-divider" />
            <div className="prop-section-title">Settings</div>
            {allFields.map((field) => (
              <div key={field.key} className="prop-section">
                {field.type !== 'boolean' && <div className="prop-label">{field.label}</div>}
                <SettingsFieldInput
                  field={field}
                  value={getSettingValue(field.key)}
                  onChange={(v) => updateSetting(field.key, v)}
                />
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

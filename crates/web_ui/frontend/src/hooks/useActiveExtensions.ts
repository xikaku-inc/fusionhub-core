import { useMemo } from 'react';
import { useAppStore } from '../stores/appStore';
import type { UiExtension } from '../types/nodes';

function getConfigKeys(config: any): Set<string> {
  const keys = new Set<string>();
  for (const section of ['sources', 'sinks']) {
    const group = config?.[section];
    if (!group || typeof group !== 'object') continue;
    for (const key of Object.keys(group)) {
      keys.add(key.replace(/^_/, ''));
    }
  }
  return keys;
}

// Find all config keys that match a required node by exact match or prefix
// e.g. requiredNode "mapSink" matches "mapSink", "mapSink_1", "mapSink_2"
function findMatchingKeys(requiredNodes: string[], activeKeys: Set<string>): string[] {
  const matches: string[] = [];
  for (const key of activeKeys) {
    for (const node of requiredNodes) {
      if (key === node || key.startsWith(node + '_')) {
        matches.push(key);
        break;
      }
    }
  }
  return matches;
}

export function useActiveExtensions(): UiExtension[] {
  const uiExtensions = useAppStore((s) => s.uiExtensions);
  const config = useAppStore((s) => s.config);

  return useMemo(() => {
    const activeKeys = getConfigKeys(config);
    const result: UiExtension[] = [];

    for (const ext of uiExtensions) {
      if (!ext.requiredNodes || ext.requiredNodes.length === 0) {
        result.push(ext);
        continue;
      }

      const matchingKeys = findMatchingKeys(ext.requiredNodes, activeKeys);
      if (matchingKeys.length === 0) continue;

      if (matchingKeys.length === 1) {
        // Single match: use original extension, tag with instanceKey
        result.push({ ...ext, instanceKey: matchingKeys[0] });
      } else {
        // Multiple matches: expand into one entry per config key
        for (const key of matchingKeys) {
          const suffix = key.replace(/^[^_]+/, '');
          const label = suffix ? suffix.replace(/^_/, ' #') : '';
          result.push({
            ...ext,
            id: `${ext.id}__${key}`,
            bundleId: ext.id,
            displayName: `${ext.displayName}${label || ' #0'}`,
            route: `${ext.route}/${key}`,
            instanceKey: key,
          });
        }
      }
    }

    return result;
  }, [uiExtensions, config]);
}

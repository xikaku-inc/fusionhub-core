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

export function useActiveExtensions(): UiExtension[] {
  const uiExtensions = useAppStore((s) => s.uiExtensions);
  const config = useAppStore((s) => s.config);

  return useMemo(() => {
    const activeKeys = getConfigKeys(config);
    return uiExtensions.filter((ext) => {
      if (!ext.requiredNodes || ext.requiredNodes.length === 0) return true;
      return ext.requiredNodes.some((node) => activeKeys.has(node));
    });
  }, [uiExtensions, config]);
}

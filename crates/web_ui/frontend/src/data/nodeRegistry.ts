import type { NodeTypeDefinition } from '../types/nodes';
import { useAppStore } from '../stores/appStore';

export function getNodeRegistry(): NodeTypeDefinition[] {
  return useAppStore.getState().nodeTypes;
}

let aliasIndex = new Map<string, NodeTypeDefinition>();
let aliasSource: NodeTypeDefinition[] = [];

function rebuildAliasIndex(types: NodeTypeDefinition[]) {
  if (types === aliasSource && aliasIndex.size > 0) return;
  aliasSource = types;
  aliasIndex = new Map();
  for (const def of types) {
    aliasIndex.set(def.id, def);
    for (const alias of def.configAliases) {
      aliasIndex.set(alias, def);
    }
  }
}

// Strip instance suffix: numeric (_1, _2) or named (_Gnss, _Odom)
function stripInstanceSuffix(key: string): string {
  if (!key) return key;
  const numStripped = key.replace(/_\d+$/, '');
  if (numStripped !== key) return numStripped;
  const lastUnderscore = key.lastIndexOf('_');
  return lastUnderscore > 0 ? key.substring(0, lastUnderscore) : key;
}

export function findNodeType(key: string): NodeTypeDefinition | undefined {
  if (!key) return undefined;
  const types = getNodeRegistry();
  rebuildAliasIndex(types);
  return aliasIndex.get(key) || aliasIndex.get(stripInstanceSuffix(key));
}

export function isFilterKey(key: string): boolean {
  if (!key) return false;
  const types = getNodeRegistry();
  rebuildAliasIndex(types);
  const def = aliasIndex.get(key) || aliasIndex.get(stripInstanceSuffix(key));
  return def?.role === 'filter';
}

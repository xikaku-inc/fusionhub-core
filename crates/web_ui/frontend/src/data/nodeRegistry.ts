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

// Strip numeric instance suffix (e.g. "mapSink_1" -> "mapSink")
function stripInstanceSuffix(key: string): string {
  return key.replace(/_\d+$/, '');
}

export function findNodeType(key: string): NodeTypeDefinition | undefined {
  const types = getNodeRegistry();
  rebuildAliasIndex(types);
  return aliasIndex.get(key) || aliasIndex.get(stripInstanceSuffix(key));
}

export function isFilterKey(key: string): boolean {
  const types = getNodeRegistry();
  rebuildAliasIndex(types);
  const def = aliasIndex.get(key) || aliasIndex.get(stripInstanceSuffix(key));
  return def?.role === 'filter';
}

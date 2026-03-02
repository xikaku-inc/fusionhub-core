import { type Node, type Edge } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';

export function graphToConfig(nodes: Node[], edges: Edge[], globalSettings: any): any {
  const config: any = {
    settings: globalSettings || {},
    sources: {},
    sinks: {},
  };

  const sourceNodes = nodes.filter((n) => (n.data as EditorNode).nodeType.role === 'source');
  const filterNodes = nodes.filter((n) => (n.data as EditorNode).nodeType.role === 'filter');
  const sinkNodes = nodes.filter((n) => (n.data as EditorNode).nodeType.role === 'sink');

  // Helper: get input endpoints for a node from edges
  const getInputEndpoints = (nodeId: string): string[] => {
    const incomingEdges = edges.filter((e) => e.target === nodeId);
    const endpoints: string[] = [];
    for (const edge of incomingEdges) {
      const sourceNode = nodes.find((n) => n.id === edge.source);
      if (sourceNode) {
        const sd = sourceNode.data as EditorNode;
        if (sd.endpoint && !endpoints.includes(sd.endpoint)) {
          endpoints.push(sd.endpoint);
        }
      }
    }
    return endpoints;
  };

  // Build sources
  // Group by original config key (handle arrays: multiple nodes with same base key)
  const sourceGroups = new Map<string, { configs: any[]; disabled: boolean }>();
  for (const node of sourceNodes) {
    const d = node.data as EditorNode;
    // Strip instance suffix (e.g., "imu_0" -> "imu")
    const baseKey = d.configKey.replace(/_\d+$/, '');
    const prefix = d.disabled ? '_' : '';
    const key = prefix + baseKey;

    const nodeConfig: any = {};
    if (d.endpoint) {
      nodeConfig.outEndpoint = d.endpoint;
    }
    if (d.subtype) {
      nodeConfig.type = d.subtype;
    }
    if (d.settings && Object.keys(d.settings).length > 0) {
      nodeConfig.settings = d.settings;
    }

    if (!sourceGroups.has(key)) {
      sourceGroups.set(key, { configs: [], disabled: d.disabled });
    }
    sourceGroups.get(key)!.configs.push(nodeConfig);
  }

  for (const [key, group] of sourceGroups) {
    config.sources[key] = group.configs.length === 1 ? group.configs[0] : group.configs;
  }

  // Build filters into sinks section
  for (const node of filterNodes) {
    const d = node.data as EditorNode;
    const prefix = d.disabled ? '_' : '';
    const key = prefix + d.configKey;

    const nodeConfig: any = {};
    if (d.endpoint) {
      nodeConfig.dataEndpoint = d.endpoint;
    }
    if (d.subtype) {
      nodeConfig.type = d.subtype;
    }

    const inputEps = getInputEndpoints(node.id);
    if (inputEps.length > 0) {
      nodeConfig.inputEndpoints = inputEps;
    }

    if (d.settings && Object.keys(d.settings).length > 0) {
      nodeConfig.settings = d.settings;
    }

    config.sinks[key] = nodeConfig;
  }

  // Build sinks
  for (const node of sinkNodes) {
    const d = node.data as EditorNode;
    const prefix = d.disabled ? '_' : '';
    const key = prefix + d.configKey;

    const nodeConfig: any = {};
    if (d.subtype) {
      nodeConfig.type = d.subtype;
    }

    const inputEps = getInputEndpoints(node.id);
    if (inputEps.length > 0) {
      nodeConfig.inputEndpoints = inputEps;
    }

    if (d.settings && Object.keys(d.settings).length > 0) {
      nodeConfig.settings = d.settings;
    }

    // Only add non-empty configs (empty object {} for sinks like echo is fine)
    config.sinks[key] = Object.keys(nodeConfig).length > 0 ? nodeConfig : {};
  }

  return config;
}

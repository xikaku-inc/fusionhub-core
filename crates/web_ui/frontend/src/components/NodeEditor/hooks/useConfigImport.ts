import { type Node, type Edge } from '@xyflow/react';
import { findNodeType, isFilterKey } from '../../../data/nodeRegistry';
import type { DataType, EditorNode, NodeTypeDefinition } from '../../../types/nodes';
import { generateEndpoint, resetEndpointCounter } from '../utils/endpointGenerator';

function normalizeEndpoint(ep: string): string {
  return ep.replace('tcp://*:', 'tcp://localhost:').replace('tcp://0.0.0.0:', 'tcp://localhost:');
}

export function configToGraph(config: any): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  const endpointToNodeId = new Map<string, { nodeId: string; outputs: DataType[] }>();
  const accumulatedEndpoints: string[] = [];
  const deferredFilterEdges: { endpoint: string; nodeId: string; nodeType: NodeTypeDefinition }[] = [];
  resetEndpointCounter();

  let sourceCol = 0;
  let filterCol = 0;
  let sinkCol = 0;

  // 1. Parse sources
  if (config.sources) {
    for (const [key, value] of Object.entries(config.sources as Record<string, any>)) {
      if (key.startsWith('_') || key === 'endpoints') continue;

      const configs: any[] = Array.isArray(value) ? value : [value];
      for (let i = 0; i < configs.length; i++) {
        const cfg = configs[i];
        const nodeId = configs.length > 1 ? `source-${key}_${i}` : `source-${key}`;
        const configKey = configs.length > 1 ? `${key}_${i}` : key;
        const nodeType = findNodeType(key);
        if (!nodeType) continue;

        const endpoint = cfg.dataEndpoint || cfg.outEndpoint || cfg.settings?.endpoints?.[0] || generateEndpoint(key, configKey);
        const normalizedEp = normalizeEndpoint(endpoint);

        // Extract subtype from config (case-insensitive match against registry)
        let subtype: string | undefined;
        const rawType = cfg.type || cfg.settings?.type;
        if (nodeType.subtypes && rawType) {
          const lower = rawType.toLowerCase();
          subtype = nodeType.subtypes.find((s) => s.value === lower)?.value || rawType;
        }

        const data: EditorNode = {
          configKey,
          nodeType,
          subtype,
          settings: cfg.settings || {},
          endpoint,
          disabled: false,
          originalConfig: cfg,
        };

        nodes.push({
          id: nodeId,
          type: 'sourceNode',
          position: { x: 50, y: sourceCol * 150 + 50 },
          data,
        });

        if (normalizedEp) {
          endpointToNodeId.set(normalizedEp, { nodeId, outputs: nodeType.outputs });
          accumulatedEndpoints.push(normalizedEp);
        }
        sourceCol++;
      }
    }

    // Handle explicit endpoints array
    if (config.sources.endpoints) {
      const eps = Array.isArray(config.sources.endpoints) ? config.sources.endpoints : [config.sources.endpoints];
      for (const ep of eps) {
        if (typeof ep === 'string') accumulatedEndpoints.push(normalizeEndpoint(ep));
      }
    }
  }

  // 2. Parse filters from sinks section
  if (config.sinks) {
    for (const [key, value] of Object.entries(config.sinks as Record<string, any>)) {
      if (key.startsWith('_') || !isFilterKey(key)) continue;

      const configs: any[] = Array.isArray(value) ? value : [value];
      for (let i = 0; i < configs.length; i++) {
        const cfg = configs[i];
        const nodeId = configs.length > 1 ? `filter-${key}_${i}` : `filter-${key}`;
        const configKey = configs.length > 1 ? `${key}_${i}` : key;
        const nodeType = findNodeType(key) || findNodeType(cfg.type);
        if (!nodeType) continue;

        const endpoint = cfg.dataEndpoint || generateEndpoint(key, configKey);
        const normalizedEp = normalizeEndpoint(endpoint);

        const data: EditorNode = {
          configKey,
          nodeType,
          subtype: cfg.type,
          settings: cfg.settings || {},
          endpoint,
          disabled: false,
          originalConfig: cfg,
        };

        nodes.push({
          id: nodeId,
          type: 'filterNode',
          position: { x: 400, y: filterCol * 150 + 50 },
          data,
        });

        // Create edges based on inputEndpoints
        const inputEndpoints: string[] = cfg.inputEndpoints || [];
        if (inputEndpoints.length > 0) {
          for (const iep of inputEndpoints) {
            const normalized = normalizeEndpoint(iep);
            const source = endpointToNodeId.get(normalized);
            if (source) {
              for (const outType of source.outputs) {
                if (nodeType.inputs.includes(outType)) {
                  edges.push({
                    id: `e-${source.nodeId}-${nodeId}-${outType}`,
                    source: source.nodeId,
                    target: nodeId,
                    sourceHandle: `out-${outType}`,
                    targetHandle: `in-${outType}`,
                    animated: false,
                  });
                }
              }
              if (!source.outputs.some((o) => nodeType.inputs.includes(o))) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: source.outputs.length > 0 ? `out-${source.outputs[0]}` : undefined,
                  targetHandle: nodeType.inputs.length > 0 ? `in-${nodeType.inputs[0]}` : undefined,
                });
              }
            } else {
              // Source not yet processed (filter ordering) — defer
              deferredFilterEdges.push({ endpoint: normalized, nodeId, nodeType });
            }
          }
        } else {
          // No explicit inputEndpoints: connect to all accumulated upstream endpoints
          for (const aep of accumulatedEndpoints) {
            const source = endpointToNodeId.get(aep);
            if (source) {
              for (const outType of source.outputs) {
                if (nodeType.inputs.includes(outType)) {
                  edges.push({
                    id: `e-${source.nodeId}-${nodeId}-${outType}`,
                    source: source.nodeId,
                    target: nodeId,
                    sourceHandle: `out-${outType}`,
                    targetHandle: `in-${outType}`,
                  });
                }
              }
            }
          }
        }

        if (normalizedEp) {
          endpointToNodeId.set(normalizedEp, { nodeId, outputs: nodeType.outputs });
          accumulatedEndpoints.push(normalizedEp);
        }
        filterCol++;
      }
    }
  }

  // 2b. Resolve deferred filter edges (filters referencing other filters processed later)
  for (const deferred of deferredFilterEdges) {
    const source = endpointToNodeId.get(deferred.endpoint);
    if (!source) continue;
    const targetNodeType = deferred.nodeType;
    for (const outType of source.outputs) {
      if (targetNodeType.inputs.includes(outType)) {
        edges.push({
          id: `e-${source.nodeId}-${deferred.nodeId}-${outType}`,
          source: source.nodeId,
          target: deferred.nodeId,
          sourceHandle: `out-${outType}`,
          targetHandle: `in-${outType}`,
        });
      }
    }
    if (!source.outputs.some((o) => targetNodeType.inputs.includes(o))) {
      edges.push({
        id: `e-${source.nodeId}-${deferred.nodeId}`,
        source: source.nodeId,
        target: deferred.nodeId,
        sourceHandle: source.outputs.length > 0 ? `out-${source.outputs[0]}` : undefined,
        targetHandle: targetNodeType.inputs.length > 0 ? `in-${targetNodeType.inputs[0]}` : undefined,
      });
    }
  }

  // 3. Parse sinks from sinks section
  if (config.sinks) {
    for (const [key, value] of Object.entries(config.sinks as Record<string, any>)) {
      if (key.startsWith('_') || isFilterKey(key)) continue;

      const cfg = value as any;
      const nodeId = `sink-${key}`;
      const nodeType = findNodeType(key) || findNodeType(cfg.type);
      if (!nodeType) continue;

      const data: EditorNode = {
        configKey: key,
        nodeType,
        subtype: cfg.type,
        settings: cfg.settings || {},
        endpoint: '',
        disabled: false,
        originalConfig: cfg,
      };

      nodes.push({
        id: nodeId,
        type: 'sinkNode',
        position: { x: 750, y: sinkCol * 150 + 50 },
        data,
      });

      // Create edges
      const inputEndpoints: string[] = cfg.inputEndpoints || [];
      if (inputEndpoints.length > 0) {
        for (const iep of inputEndpoints) {
          const normalized = normalizeEndpoint(iep);
          const source = endpointToNodeId.get(normalized);
          if (source) {
            const hasSpecificInputs = nodeType.inputs.length > 0;
            for (const outType of source.outputs) {
              if (!hasSpecificInputs || nodeType.inputs.includes(outType)) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}-${outType}`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: `out-${outType}`,
                  targetHandle: hasSpecificInputs ? `in-${outType}` : 'in-any',
                });
              }
            }
          }
        }
      } else {
        // Connect to all accumulated endpoints
        for (const aep of accumulatedEndpoints) {
          const source = endpointToNodeId.get(aep);
          if (source) {
            const hasSpecificInputs = nodeType.inputs.length > 0;
            for (const outType of source.outputs) {
              if (!hasSpecificInputs || nodeType.inputs.includes(outType)) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}-${outType}`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: `out-${outType}`,
                  targetHandle: hasSpecificInputs ? `in-${outType}` : 'in-any',
                });
              }
            }
          }
        }
      }

      sinkCol++;
    }
  }

  return { nodes, edges };
}

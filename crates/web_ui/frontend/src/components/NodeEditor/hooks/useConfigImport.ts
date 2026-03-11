import { type Node, type Edge } from '@xyflow/react';
import { findNodeType, isFilterKey } from '../../../data/nodeRegistry';
import type { DataType, EditorNode, NodeTypeDefinition } from '../../../types/nodes';
import { generateEndpoint, resetEndpointCounter } from '../utils/endpointGenerator';
import {
  EXTERNAL_INPUT_TYPE,
  EXTERNAL_OUTPUT_TYPE,
  sanitizeEndpointId,
  isTcpEndpoint,
} from '../utils/externalNodeTypes';

function normalizeEndpoint(ep: string): string {
  return ep.replace('tcp://*:', 'tcp://localhost:').replace('tcp://0.0.0.0:', 'tcp://localhost:');
}

function getOrCreateExternalInput(
  endpoint: string,
  normalizedEp: string,
  nodes: Node[],
  endpointToNodeId: Map<string, { nodeId: string; outputs: DataType[] }>,
  externalInputNodes: Map<string, string>,
  extInputCol: { v: number },
): string {
  const existing = externalInputNodes.get(normalizedEp);
  if (existing) return existing;

  const nodeId = `ext-in-${sanitizeEndpointId(normalizedEp)}`;
  const data: EditorNode = {
    configKey: nodeId,
    nodeType: EXTERNAL_INPUT_TYPE,
    settings: {},
    endpoint,
    disabled: false,
    externalDirection: 'input',
  };
  nodes.push({
    id: nodeId,
    type: 'externalNode',
    position: { x: -200, y: extInputCol.v * 100 + 50 },
    data,
  });
  endpointToNodeId.set(normalizedEp, { nodeId, outputs: [] });
  externalInputNodes.set(normalizedEp, nodeId);
  extInputCol.v++;
  return nodeId;
}

export function configToGraph(config: any): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  const endpointToNodeId = new Map<string, { nodeId: string; outputs: DataType[] }>();
  const accumulatedEndpoints: string[] = [];
  const explicitConnections = config.settings?.explicitConnections === true;
  const deferredFilterEdges: { endpoint: string; rawEndpoint: string; nodeId: string; nodeType: NodeTypeDefinition }[] = [];
  const externalInputNodes = new Map<string, string>();
  resetEndpointCounter();

  let sourceCol = 0;
  let filterCol = 0;
  let sinkCol = 0;
  const extInputCol = { v: 0 };
  let extOutputCol = 0;

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

    // Create edges for sources with inputEndpoints (e.g. NMEA receiving RTCM)
    for (const [key, value] of Object.entries(config.sources as Record<string, any>)) {
      if (key.startsWith('_') || key === 'endpoints') continue;
      const configs: any[] = Array.isArray(value) ? value : [value];
      for (let i = 0; i < configs.length; i++) {
        const cfg = configs[i];
        const inputEndpoints: string[] = cfg.inputEndpoints || [];
        if (inputEndpoints.length === 0) continue;
        const nodeId = configs.length > 1 ? `source-${key}_${i}` : `source-${key}`;
        const nodeType = findNodeType(key);
        if (!nodeType) continue;
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
                });
              }
            }
          }
        }
      }
    }

    // Handle explicit endpoints array — create external input nodes for TCP entries
    if (config.sources.endpoints) {
      const eps = Array.isArray(config.sources.endpoints) ? config.sources.endpoints : [config.sources.endpoints];
      for (const ep of eps) {
        if (typeof ep !== 'string') continue;
        const normalized = normalizeEndpoint(ep);
        accumulatedEndpoints.push(normalized);

        if (isTcpEndpoint(ep)) {
          getOrCreateExternalInput(ep, normalized, nodes, endpointToNodeId, externalInputNodes, extInputCol);
        }
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
              // Fallback: external input nodes have empty outputs, create untyped edge
              if (source.outputs.length === 0) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}-ext`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: 'out-ext',
                  targetHandle: nodeType.inputs.length > 0 ? `in-${nodeType.inputs[0]}` : 'in-any',
                });
              } else if (!source.outputs.some((o) => nodeType.inputs.includes(o))) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: source.outputs.length > 0 ? `out-${source.outputs[0]}` : undefined,
                  targetHandle: nodeType.inputs.length > 0 ? `in-${nodeType.inputs[0]}` : undefined,
                });
              }
            } else if (isTcpEndpoint(iep)) {
              // Unresolved TCP endpoint — create external input node
              const extNodeId = getOrCreateExternalInput(iep, normalized, nodes, endpointToNodeId, externalInputNodes, extInputCol);
              edges.push({
                id: `e-${extNodeId}-${nodeId}-ext`,
                source: extNodeId,
                target: nodeId,
                sourceHandle: 'out-ext',
                targetHandle: nodeType.inputs.length > 0 ? `in-${nodeType.inputs[0]}` : 'in-any',
              });
            } else {
              // Source not yet processed (filter ordering) — defer
              deferredFilterEdges.push({ endpoint: normalized, rawEndpoint: iep, nodeId, nodeType });
            }
          }
        } else if (!explicitConnections) {
          // Legacy: no explicit inputEndpoints — connect to all accumulated upstream endpoints
          for (const aep of accumulatedEndpoints) {
            const source = endpointToNodeId.get(aep);
            if (source) {
              if (source.outputs.length === 0) {
                edges.push({
                  id: `e-${source.nodeId}-${nodeId}-ext`,
                  source: source.nodeId,
                  target: nodeId,
                  sourceHandle: 'out-ext',
                  targetHandle: nodeType.inputs.length > 0 ? `in-${nodeType.inputs[0]}` : 'in-any',
                });
              } else {
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
    if (source) {
      const targetNodeType = deferred.nodeType;
      if (source.outputs.length === 0) {
        edges.push({
          id: `e-${source.nodeId}-${deferred.nodeId}-ext`,
          source: source.nodeId,
          target: deferred.nodeId,
          sourceHandle: 'out-ext',
          targetHandle: targetNodeType.inputs.length > 0 ? `in-${targetNodeType.inputs[0]}` : 'in-any',
        });
      } else {
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
    } else if (isTcpEndpoint(deferred.rawEndpoint)) {
      // Still unresolved TCP endpoint — create external input node
      const extNodeId = getOrCreateExternalInput(
        deferred.rawEndpoint, deferred.endpoint, nodes, endpointToNodeId, externalInputNodes, extInputCol,
      );
      edges.push({
        id: `e-${extNodeId}-${deferred.nodeId}-ext`,
        source: extNodeId,
        target: deferred.nodeId,
        sourceHandle: 'out-ext',
        targetHandle: deferred.nodeType.inputs.length > 0 ? `in-${deferred.nodeType.inputs[0]}` : 'in-any',
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
            if (source.outputs.length === 0) {
              edges.push({
                id: `e-${source.nodeId}-${nodeId}-ext`,
                source: source.nodeId,
                target: nodeId,
                sourceHandle: 'out-ext',
                targetHandle: hasSpecificInputs ? `in-${nodeType.inputs[0]}` : 'in-any',
              });
            } else {
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
          } else if (isTcpEndpoint(iep)) {
            // Unresolved TCP endpoint — create external input node
            const extNodeId = getOrCreateExternalInput(iep, normalized, nodes, endpointToNodeId, externalInputNodes, extInputCol);
            const hasSpecificInputs = nodeType.inputs.length > 0;
            edges.push({
              id: `e-${extNodeId}-${nodeId}-ext`,
              source: extNodeId,
              target: nodeId,
              sourceHandle: 'out-ext',
              targetHandle: hasSpecificInputs ? `in-${nodeType.inputs[0]}` : 'in-any',
            });
          }
        }
      } else if (!explicitConnections) {
        // Legacy: connect to all accumulated endpoints
        for (const aep of accumulatedEndpoints) {
          const source = endpointToNodeId.get(aep);
          if (source) {
            const hasSpecificInputs = nodeType.inputs.length > 0;
            if (source.outputs.length === 0) {
              edges.push({
                id: `e-${source.nodeId}-${nodeId}-ext`,
                source: source.nodeId,
                target: nodeId,
                sourceHandle: 'out-ext',
                targetHandle: hasSpecificInputs ? `in-${nodeType.inputs[0]}` : 'in-any',
              });
            } else {
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
      }

      sinkCol++;
    }
  }

  // 4. Create external output nodes for TCP dataEndpoints
  const extOutputSeen = new Set<string>();
  const internalNodes = nodes.filter((n) => !(n.data as EditorNode).externalDirection);
  for (const node of internalNodes) {
    const d = node.data as EditorNode;
    if (!d.endpoint || !isTcpEndpoint(d.endpoint)) continue;

    const normalized = normalizeEndpoint(d.endpoint);
    if (extOutputSeen.has(normalized)) continue;
    extOutputSeen.add(normalized);

    const extNodeId = `ext-out-${sanitizeEndpointId(normalized)}`;
    const extData: EditorNode = {
      configKey: extNodeId,
      nodeType: EXTERNAL_OUTPUT_TYPE,
      settings: {},
      endpoint: d.endpoint,
      disabled: false,
      externalDirection: 'output',
    };

    nodes.push({
      id: extNodeId,
      type: 'externalNode',
      position: { x: 1000, y: extOutputCol * 100 + 50 },
      data: extData,
    });

    edges.push({
      id: `e-${node.id}-${extNodeId}-ext`,
      source: node.id,
      target: extNodeId,
      sourceHandle: d.nodeType.outputs.length > 0 ? `out-${d.nodeType.outputs[0]}` : undefined,
      targetHandle: 'in-ext',
    });
    extOutputCol++;
  }

  // Restore node positions from layout
  const savedPositions: Record<string, { x: number; y: number }> = config._layout?.nodePositions || {};
  for (let i = 0; i < nodes.length; i++) {
    const pos = savedPositions[nodes[i].id];
    if (pos) nodes[i] = { ...nodes[i], position: pos };
  }

  return { nodes, edges };
}

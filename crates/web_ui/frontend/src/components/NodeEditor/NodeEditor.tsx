import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ReactFlow,
  Controls,
  Background,
  MiniMap,
  Panel,
  addEdge,
  useNodesState,
  useEdgesState,
  type Connection,
  type Node,
  type Edge,
  BackgroundVariant,
  type ReactFlowInstance,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import './NodeEditor.css';
import { useAppStore } from '../../stores/appStore';
import { configToGraph } from './hooks/useConfigImport';
import { graphToConfig } from './hooks/useConfigExport';
import { isValidConnection } from './utils/connectionValidator';
import { generateEndpoint } from './utils/endpointGenerator';
import { getNodeRate } from './utils/nodeStatusMapper';
import SourceNode from './nodes/SourceNode';
import FilterNode from './nodes/FilterNode';
import SinkNode from './nodes/SinkNode';
import ExternalNode from './nodes/ExternalNode';
import NodePalette from './NodePalette';
import PropertiesPanel from './PropertiesPanel';
import OscilloscopePanel from './OscilloscopePanel';
import type { NodeTypeDefinition, EditorNode } from '../../types/nodes';
import { apiPost } from '../../api/client';

const nodeTypes = {
  sourceNode: SourceNode,
  filterNode: FilterNode,
  sinkNode: SinkNode,
  externalNode: ExternalNode,
};

const nodeTypeToFlowType: Record<string, string> = {
  source: 'sourceNode',
  filter: 'filterNode',
  sink: 'sinkNode',
};

export default function NodeEditor() {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [selectedNode, setSelectedNode] = useState<Node | null>(null);
  const [oscilloscopeEdge, setOscilloscopeEdge] = useState<Edge | null>(null);
  const [scopeHeight, setScopeHeight] = useState(220);
  const dragRef = useRef<{ startY: number; startH: number } | null>(null);
  const config = useAppStore((s) => s.config);
  const inputRates = useAppStore((s) => s.inputRates);
  const fusionRates = useAppStore((s) => s.fusionRates);
  const nodeRates = useAppStore((s) => s.nodeRates);
  const [explicitConnections, setExplicitConnections] = useState(false);
  const reactFlowWrapper = useRef<HTMLDivElement>(null);
  const [reactFlowInstance, setReactFlowInstance] = useState<ReactFlowInstance | null>(null);
  const importedRef = useRef(false);
  const nodesRef = useRef(nodes);
  nodesRef.current = nodes;
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Import config on first load
  useEffect(() => {
    if (config && Object.keys(config).length > 0 && !importedRef.current) {
      importedRef.current = true;
      setExplicitConnections(config.settings?.explicitConnections === true);
      const { nodes: n, edges: e } = configToGraph(config);
      setNodes(n);
      setEdges(e);
    }
  }, [config, setNodes, setEdges]);

  // Re-import graph when explicit connections toggle changes
  useEffect(() => {
    if (config && importedRef.current) {
      const modifiedConfig = {
        ...config,
        settings: { ...config.settings, explicitConnections },
      };
      const { nodes: n, edges: e } = configToGraph(modifiedConfig);
      setNodes(n);
      setEdges(e);
      setSelectedNode(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [explicitConnections]);

  // Animate edges and propagate active status through graph
  useEffect(() => {
    // Use functional updaters to avoid depending on nodes/edges state directly
    setEdges((eds) => {
      const activeNodes = new Set<string>();
      for (const n of nodesRef.current) {
        const d = n.data as EditorNode;
        if (d.externalDirection) continue; // handled below
        // Use per-node rates from nodeStatuses SSE (covers all connection types)
        const nr = nodeRates[d.configKey];
        if (nr && (nr.inputRate > 0 || nr.outputRate > 0)) {
          activeNodes.add(n.id);
          continue;
        }
        // Fallback to legacy hardcoded rate maps
        const rate = getNodeRate(d.nodeType.id, d.configKey, d.nodeType.role, inputRates, fusionRates);
        if (rate > 0) activeNodes.add(n.id);
      }
      // Propagate forward through edges
      let changed = true;
      while (changed) {
        changed = false;
        for (const edge of eds) {
          if (activeNodes.has(edge.source) && !activeNodes.has(edge.target)) {
            activeNodes.add(edge.target);
            changed = true;
          }
        }
      }
      // Reverse-propagate: external input nodes are active if any downstream target is active
      const extInputIds = new Set(
        nodesRef.current
          .filter((n) => (n.data as EditorNode).externalDirection === 'input')
          .map((n) => n.id),
      );
      for (const edge of eds) {
        if (extInputIds.has(edge.source) && activeNodes.has(edge.target)) {
          activeNodes.add(edge.source);
        }
      }
      // Update node active flags
      setNodes((nds) =>
        nds.map((n) => {
          const isActive = activeNodes.has(n.id);
          if ((n.data as EditorNode).active === isActive) return n;
          return { ...n, data: { ...n.data, active: isActive } };
        }),
      );
      return eds.map((edge) => {
        const animated = activeNodes.has(edge.source);
        if (edge.animated === animated) return edge;
        return { ...edge, animated };
      });
    });
  // Only re-run when rates change, not when nodes/edges change
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inputRates, fusionRates, nodeRates]);

  const onConnect = useCallback(
    (connection: Connection) => {
      if (isValidConnection(connection, nodes, edges)) {
        setEdges((eds) => addEdge(connection, eds));
      }
    },
    [nodes, edges, setEdges],
  );

  const onNodeClick = useCallback(
    (_: any, node: Node) => {
      setSelectedNode(node);
    },
    [],
  );

  const onPaneClick = useCallback(() => {
    setSelectedNode(null);
  }, []);

  const onEdgeClick = useCallback((_: any, edge: Edge) => {
    setOscilloscopeEdge(edge);
  }, []);

  const onUpdateNode = useCallback(
    (id: string, updates: Partial<EditorNode>) => {
      setNodes((nds) =>
        nds.map((n) => {
          if (n.id === id) {
            return { ...n, data: { ...n.data, ...updates } };
          }
          return n;
        }),
      );
      setSelectedNode((prev) => {
        if (prev && prev.id === id) {
          return { ...prev, data: { ...prev.data, ...updates } };
        }
        return prev;
      });
    },
    [setNodes],
  );

  const handleImport = useCallback(() => {
    if (config && Object.keys(config).length > 0) {
      const { nodes: n, edges: e } = configToGraph(config);
      setNodes(n);
      setEdges(e);
      setSelectedNode(null);
    }
  }, [config, setNodes, setEdges]);

  const handleExport = useCallback(async () => {
    const settings = { ...(config?.settings || {}), explicitConnections };
    const newConfig = graphToConfig(nodes, edges, settings);
    if (config?.LicenseInfo) {
      newConfig.LicenseInfo = config.LicenseInfo;
    }
    try {
      await apiPost('/api/config', newConfig);
      useAppStore.getState().setConfig(newConfig);
      await apiPost('/api/config/save');
    } catch (e) {
      console.error('Failed to save config:', e);
    }
  }, [nodes, edges, config, explicitConnections]);

  const handleLoadFile = useCallback(async (ev: React.ChangeEvent<HTMLInputElement>) => {
    const file = ev.target.files?.[0];
    if (!file) return;
    ev.target.value = '';
    try {
      const text = await file.text();
      const loaded = JSON.parse(text);
      await apiPost('/api/config', loaded);
      useAppStore.getState().setConfig(loaded);
      const graph = configToGraph(loaded);
      setNodes(graph.nodes);
      setEdges(graph.edges);
      setSelectedNode(null);
    } catch (err: any) {
      alert('Failed to load config: ' + err.message);
    }
  }, [setNodes, setEdges]);

  const handleSaveAs = useCallback(() => {
    const settings = { ...(config?.settings || {}), explicitConnections };
    const newConfig = graphToConfig(nodes, edges, settings);
    if (config?.LicenseInfo) {
      newConfig.LicenseInfo = config.LicenseInfo;
    }
    const json = JSON.stringify(newConfig, null, 2);
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'config.json';
    a.click();
    URL.revokeObjectURL(url);
  }, [nodes, edges, config, explicitConnections]);

  const handleDeleteSelected = useCallback(() => {
    setNodes((nds) => nds.filter((n) => !n.selected));
    setEdges((eds) => {
      const selectedNodeIds = new Set(nodes.filter((n) => n.selected).map((n) => n.id));
      return eds.filter((e) => !e.selected && !selectedNodeIds.has(e.source) && !selectedNodeIds.has(e.target));
    });
    setSelectedNode(null);
  }, [nodes, setNodes, setEdges]);

  const onDragOver = useCallback((event: React.DragEvent) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
  }, []);

  const onDrop = useCallback(
    (event: React.DragEvent) => {
      event.preventDefault();
      const data = event.dataTransfer.getData('application/fusionhub-node');
      if (!data || !reactFlowInstance) return;

      const nodeType: NodeTypeDefinition = JSON.parse(data);
      const position = reactFlowInstance.screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      });

      // Handle external node types
      if (nodeType.id === '_external_input' || nodeType.id === '_external_output') {
        const direction = nodeType.id === '_external_input' ? 'input' : 'output';
        const endpoint = `tcp://localhost:${direction === 'input' ? 8900 : 8901}`;
        const nodeId = `ext-${direction === 'input' ? 'in' : 'out'}-${Date.now()}`;

        const newNode: Node = {
          id: nodeId,
          type: 'externalNode',
          position,
          data: {
            configKey: nodeId,
            nodeType,
            settings: {},
            endpoint,
            disabled: false,
            externalDirection: direction,
          } as EditorNode,
        };
        setNodes((nds) => [...nds, newNode]);
        return;
      }

      const existingKeys = nodes.map((n) => (n.data as EditorNode).configKey);
      let configKey = nodeType.id;
      let counter = 1;
      while (existingKeys.includes(configKey)) {
        configKey = `${nodeType.id}_${counter}`;
        counter++;
      }

      const endpoint = nodeType.role !== 'sink' ? generateEndpoint(nodeType.id, configKey) : '';
      const subtype = nodeType.subtypes?.[0]?.value;

      const newNode: Node = {
        id: `${nodeTypeToFlowType[nodeType.role].replace('Node', '')}-${configKey}`,
        type: nodeTypeToFlowType[nodeType.role],
        position,
        data: {
          configKey,
          nodeType,
          subtype,
          settings: { ...nodeType.defaultSettings },
          endpoint,
          disabled: false,
        } as EditorNode,
      };

      setNodes((nds) => [...nds, newNode]);
    },
    [reactFlowInstance, nodes, setNodes],
  );

  const connectionValidator = useCallback(
    (connection: Edge | Connection) => isValidConnection(connection as Connection, nodes, edges),
    [nodes, edges],
  );

  const onResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragRef.current = { startY: e.clientY, startH: scopeHeight };
    const onMove = (ev: MouseEvent) => {
      if (!dragRef.current) return;
      const delta = dragRef.current.startY - ev.clientY;
      setScopeHeight(Math.max(100, Math.min(600, dragRef.current.startH + delta)));
    };
    const onUp = () => {
      dragRef.current = null;
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }, [scopeHeight]);

  return (
    <div className="node-editor-container">
      <NodePalette />
      <div className="node-editor-main">
        <div
          className="node-editor-canvas"
          ref={reactFlowWrapper}
          style={oscilloscopeEdge ? { height: `calc(100% - ${scopeHeight}px)` } : undefined}
        >
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            onNodeClick={onNodeClick}
            onPaneClick={onPaneClick}
            onEdgeClick={onEdgeClick}
            isValidConnection={connectionValidator}
            nodeTypes={nodeTypes}
            onInit={setReactFlowInstance}
            onDragOver={onDragOver}
            onDrop={onDrop}
            deleteKeyCode="Delete"
            fitView
            colorMode="dark"
          >
            <Controls />
            <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
            <MiniMap
              nodeStrokeWidth={3}
              nodeColor={(n) => {
                const d = n.data as EditorNode;
                return d.nodeType?.color || '#666';
              }}
            />
            <Panel position="top-right">
              <div className="flex-row">
                <label className="checkbox" style={{ whiteSpace: 'nowrap' }}>
                  <input
                    type="checkbox"
                    checked={explicitConnections}
                    onChange={(e) => setExplicitConnections(e.target.checked)}
                  />
                  Explicit connections
                </label>
                <button className="secondary" onClick={() => fileInputRef.current?.click()}>
                  Load...
                </button>
                <input ref={fileInputRef} type="file" accept=".json" style={{ display: 'none' }} onChange={handleLoadFile} />
                <button className="secondary" onClick={handleImport}>
                  Reload
                </button>
                <button onClick={handleExport}>
                  Apply &amp; Save
                </button>
                <button className="secondary" onClick={handleSaveAs}>
                  Save As...
                </button>
                <button className="danger" onClick={handleDeleteSelected}>
                  Delete Selected
                </button>
              </div>
            </Panel>
          </ReactFlow>
        </div>
        {oscilloscopeEdge && (
          <>
            <div className="oscilloscope-resize-handle" onMouseDown={onResizeStart} />
            <OscilloscopePanel
              edge={oscilloscopeEdge}
              nodes={nodes}
              onClose={() => setOscilloscopeEdge(null)}
              height={scopeHeight}
            />
          </>
        )}
      </div>
      <PropertiesPanel node={selectedNode} onUpdateNode={onUpdateNode} />
    </div>
  );
}

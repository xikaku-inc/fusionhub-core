import { useCallback, useEffect, useRef, useState } from 'react';
import type { Node, Edge } from '@xyflow/react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';
import { useAppStore } from '../../stores/appStore';
import { apiPost } from '../../api/client';
import { getData, getGeneration, clearBuffer } from './oscilloscopeBuffer';
import type { EditorNode } from '../../types/nodes';

interface OscilloscopePanelProps {
  edge: Edge;
  nodes: Node[];
  onClose: () => void;
  height?: number;
}

interface DetectedType {
  dataType: string;
  fields: string[];
}

export default function OscilloscopePanel({ edge, nodes, onClose, height }: OscilloscopePanelProps) {
  const [resolvedType, setResolvedType] = useState('');
  const [fields, setFields] = useState<string[]>([]);
  const [selectedField, setSelectedField] = useState('');
  const [loading, setLoading] = useState(false);
  const chartRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);

  const rawDataType = edge.sourceHandle?.replace('out-', '') || '';
  const isDynamic = rawDataType === 'Dynamic';
  const sourceNode = nodes.find((n) => n.id === edge.source);
  const endpoint = (sourceNode?.data as EditorNode)?.endpoint || '';

  // For Dynamic: store types are the single source of truth (initial + discovery)
  const storeTypes = useAppStore((s) => s.oscilloscopeTypes) as DetectedType[];

  // Trigger initial detect (starts discovery in background)
  useEffect(() => {
    if (!rawDataType || !endpoint) return;
    setFields([]);
    setSelectedField('');
    setResolvedType('');

    if (isDynamic) {
      setLoading(true);
      useAppStore.getState().setOscilloscopeTypes([]);
      apiPost('/api/oscilloscope/detect', { endpoint })
        .catch(() => {})
        .finally(() => setLoading(false));
    } else {
      setResolvedType(rawDataType);
      setLoading(true);
      apiPost<{ fields: string[] }>('/api/oscilloscope/fields', { dataType: rawDataType })
        .then((res) => {
          setFields(res.fields);
          if (res.fields.length > 0) setSelectedField(res.fields[0]);
        })
        .catch(() => setFields([]))
        .finally(() => setLoading(false));
    }
  }, [rawDataType, endpoint, isDynamic]);

  // React to store type updates (initial probe + discovery additions)
  useEffect(() => {
    if (!isDynamic || storeTypes.length === 0) return;
    // Auto-select first type if nothing selected yet
    if (!resolvedType) {
      setResolvedType(storeTypes[0].dataType);
      setFields(storeTypes[0].fields);
      if (storeTypes[0].fields.length > 0) {
        setSelectedField(storeTypes[0].fields[0]);
      }
    }
  }, [storeTypes, isDynamic, resolvedType]);

  const handleTypeChange = useCallback((newType: string) => {
    setResolvedType(newType);
    const entry = storeTypes.find((t) => t.dataType === newType);
    if (entry) {
      setFields(entry.fields);
      setSelectedField(entry.fields[0] || '');
    }
  }, [storeTypes]);

  // Start/stop probe when field selection changes
  useEffect(() => {
    if (!selectedField || !endpoint || !resolvedType) return;
    clearBuffer();
    apiPost('/api/oscilloscope/start', {
      endpoint,
      dataType: resolvedType,
      field: selectedField,
      maxRate: 60,
    });
    return () => {
      apiPost('/api/oscilloscope/stop');
    };
  }, [endpoint, resolvedType, selectedField]);

  // Initialize uPlot
  useEffect(() => {
    if (!chartRef.current) return;

    const opts: uPlot.Options = {
      width: chartRef.current.clientWidth,
      height: chartRef.current.clientHeight - 4,
      cursor: { show: false },
      select: { show: false, left: 0, top: 0, width: 0, height: 0 },
      legend: { show: false },
      axes: [
        {
          stroke: '#888',
          grid: { stroke: 'rgba(255,255,255,0.07)', width: 1 },
          ticks: { stroke: 'rgba(255,255,255,0.1)', width: 1 },
          values: (_u, vals) => vals.map((v) => {
            if (v == null) return '';
            const d = new Date(v * 1000);
            return d.toLocaleTimeString([], { hour12: false, minute: '2-digit', second: '2-digit' });
          }),
        },
        {
          stroke: '#888',
          grid: { stroke: 'rgba(255,255,255,0.07)', width: 1 },
          ticks: { stroke: 'rgba(255,255,255,0.1)', width: 1 },
          size: 60,
        },
      ],
      series: [
        {},
        {
          stroke: '#4fc3f7',
          width: 1.5,
          fill: 'rgba(79,195,247,0.08)',
        },
      ],
    };

    const plot = new uPlot(opts, [[], []], chartRef.current);
    plotRef.current = plot;

    const handleResize = () => {
      if (chartRef.current && plotRef.current) {
        plotRef.current.setSize({
          width: chartRef.current.clientWidth,
          height: chartRef.current.clientHeight - 4,
        });
      }
    };
    const observer = new ResizeObserver(handleResize);
    observer.observe(chartRef.current);

    return () => {
      observer.disconnect();
      plot.destroy();
      plotRef.current = null;
    };
  }, []);

  // Poll buffer and update chart at ~20fps
  useEffect(() => {
    let lastGen = 0;
    const id = setInterval(() => {
      if (!plotRef.current) return;
      const g = getGeneration();
      if (g === lastGen) return;
      lastGen = g;
      const [t, v] = getData();
      plotRef.current.setData([t, v]);
    }, 50);
    return () => clearInterval(id);
  }, []);

  const handleClose = useCallback(() => {
    apiPost('/api/oscilloscope/stop');
    clearBuffer();
    onClose();
  }, [onClose]);

  const typeList = isDynamic ? storeTypes : [];

  return (
    <div className="oscilloscope-panel" style={height ? { height } : undefined}>
      <div className="oscilloscope-header">
        {isDynamic && typeList.length >= 1 ? (
          <select
            className="oscilloscope-type-select"
            value={resolvedType}
            onChange={(e) => handleTypeChange(e.target.value)}
          >
            {typeList.map((t) => (
              <option key={t.dataType} value={t.dataType}>{t.dataType}</option>
            ))}
          </select>
        ) : (
          <span className="oscilloscope-type">{resolvedType || rawDataType}</span>
        )}
        <select
          className="oscilloscope-field-select"
          value={selectedField}
          onChange={(e) => setSelectedField(e.target.value)}
          disabled={loading || fields.length === 0}
        >
          {fields.map((f) => (
            <option key={f} value={f}>{f}</option>
          ))}
        </select>
        {loading && <span className="oscilloscope-warn">Detecting...</span>}
        {!endpoint && <span className="oscilloscope-warn">No endpoint</span>}
        <button className="oscilloscope-close" onClick={handleClose}>
          &times;
        </button>
      </div>
      <div className="oscilloscope-chart" ref={chartRef} />
    </div>
  );
}

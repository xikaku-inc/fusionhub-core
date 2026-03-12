import { create } from 'zustand';
import { apiGet, apiPost, apiPostFormData } from '../api/client';
import type { FusionStatus, InputStatus, IntercalibrationStatus, FusedPose, OpticalData, FusedVehiclePose, LicenseInfo, InputRates, FusionRates, NodeStatuses, NodeRateEntry, NodeStatusPayload, McpStatus, LogEntry, NodeConsoleEntry } from '../api/types';
import type { NodeTypeDefinition, UiExtension } from '../types/nodes';

interface AppState {
  // Node Types
  nodeTypes: NodeTypeDefinition[];
  nodeTypesLoaded: boolean;
  fetchNodeTypes: () => Promise<void>;

  // UI Extensions
  uiExtensions: UiExtension[];
  uiExtensionsLoaded: boolean;
  fetchUiExtensions: () => Promise<void>;
  registerExtensionComponent: (id: string, component: React.ComponentType) => void;

  // Connection
  sseConnected: boolean;
  setSseConnected: (v: boolean) => void;

  // Config
  config: any;
  configText: string;
  configError: string;
  setConfig: (c: any) => void;
  setConfigText: (t: string) => void;
  refreshConfig: () => Promise<void>;
  setConfigFromEditor: () => Promise<void>;
  saveConfig: () => Promise<void>;

  // Status
  status: FusionStatus;
  inputStatus: InputStatus;
  intercalibrationStatus: IntercalibrationStatus;
  fusedPose: FusedPose;
  fusedVehiclePose: FusedVehiclePose;
  opticalData: OpticalData;
  setStatus: (s: FusionStatus) => void;
  setInputStatus: (s: InputStatus) => void;
  setIntercalibrationStatus: (s: IntercalibrationStatus) => void;
  setFusedPose: (p: FusedPose) => void;
  setFusedVehiclePose: (p: FusedVehiclePose) => void;
  setOpticalData: (d: OpticalData) => void;

  // Rates
  inputRates: InputRates;
  fusionRates: FusionRates;
  setInputRates: (r: InputRates) => void;
  setFusionRates: (r: FusionRates) => void;

  // Node statuses
  paused: boolean;
  nodeStatuses: NodeStatuses;
  nodeRates: Record<string, NodeRateEntry>;
  setNodeStatuses: (payload: NodeStatusPayload) => void;
  restart: () => Promise<void>;
  togglePause: () => Promise<void>;

  // MCP
  mcpStatus: McpStatus | null;
  setMcpStatus: (s: McpStatus) => void;

  // AI Monitor
  aiMonitorStatus: any;
  setAiMonitorStatus: (s: any) => void;

  // Oscilloscope
  oscilloscopeData: { t: number[]; v: number[] };
  pushOscilloscopePoint: (t: number, v: number) => void;
  clearOscilloscopeData: () => void;
  oscilloscopeTypes: { dataType: string; fields: string[] }[];
  setOscilloscopeTypes: (types: { dataType: string; fields: string[] }[]) => void;

  // Logs
  logEntries: LogEntry[];
  addLogEntries: (entries: LogEntry[]) => void;
  clearLogEntries: () => void;

  // Node console logs
  nodeConsoleLogs: Record<string, NodeConsoleEntry[]>;
  clearNodeConsoleLogs: (nodeKey?: string) => void;

  // License
  license: {
    info: LicenseInfo;
    method: string;
    licenseFile: string;
    licenseKey: string;
    serverUrl: string;
    loading: boolean;
    message: string;
    messageType: string;
    machines: any[];
    machinesMax: number;
    machinesLoading: boolean;
    machinesError: string;
  };
  setLicenseInfo: (info: LicenseInfo) => void;
  setLicenseField: (field: string, value: any) => void;
  fetchLicenseStatus: () => Promise<void>;
  checkLicenseFile: () => Promise<void>;
  checkLicenseServer: () => Promise<void>;
  checkLicenseToken: () => Promise<void>;
  uploadLicense: (file: File) => Promise<void>;
  fetchMachines: () => Promise<void>;
  deactivateMachine: (machineCode: string) => Promise<void>;
}

const defaultFusionStatus: FusionStatus = {
  autocalibrationDurationSinceLast: 0,
  autocalibrationStatus: false,
  autocalibrationUpdated: false,
  autocalibrationValue: null,
  matcherStatus: { finished: false, nPoses: 0, nTotalPoses: [0, 0], nUsedPoses: 0, quat: { w: 1, x: 0, y: 0, z: 0 } },
  nImu: 0, nOptical: 0, sensorMotionStatus: 0,
};

const defaultInputStatus: InputStatus = { nImu: 0, nOptical: 0, nGnss: 0, nCan: 0, nRtcmData: 0, nVehicleSpeed: 0, gnssQuality: 0 };

const defaultIntercalibrationStatus: IntercalibrationStatus = {
  nPoses: 0, nTotalPoses: [0, 0], nUsedPoses: 0, quat: null, trans: null, minNPoses: 0, finished: false, isRunning: false,
};

const defaultLicenseInfo: LicenseInfo = {
  valid: false, status: 'not_checked', customer: '', product: '', features: [], expires: null, lease_expires: null, machine_code: '', license_key: '', error: '',
};

export const useAppStore = create<AppState>((set, get) => ({
  // Node Types
  nodeTypes: [],
  nodeTypesLoaded: false,
  fetchNodeTypes: async () => {
    try {
      const data = await apiGet<NodeTypeDefinition[]>('/api/node-types');
      set({ nodeTypes: data, nodeTypesLoaded: true });
    } catch {
      set({ nodeTypesLoaded: true });
    }
  },

  // UI Extensions
  uiExtensions: [],
  uiExtensionsLoaded: false,
  fetchUiExtensions: async () => {
    try {
      const data = await apiGet<UiExtension[]>('/api/ui-extensions');
      set({ uiExtensions: data, uiExtensionsLoaded: true });
    } catch {
      set({ uiExtensionsLoaded: true });
    }
  },
  registerExtensionComponent: (id, component) => {
    set((s) => ({
      uiExtensions: s.uiExtensions.map((ext) =>
        ext.id === id ? { ...ext, component } : ext
      ),
    }));
  },

  // Connection
  sseConnected: false,
  setSseConnected: (v) => set({ sseConnected: v }),

  // Config
  config: {},
  configText: '',
  configError: '',
  setConfig: (c) => set({ config: c, configText: JSON.stringify(c, null, 4) }),
  setConfigText: (t) => set({ configText: t }),
  refreshConfig: async () => {
    try {
      const data = await apiGet('/api/config');
      set({ config: data, configText: JSON.stringify(data, null, 4), configError: '' });
    } catch {
      set({ configError: 'Failed to fetch config' });
    }
  },
  setConfigFromEditor: async () => {
    const { configText } = get();
    try {
      const parsed = JSON.parse(configText);
      set({ configError: '' });
      await apiPost('/api/config', parsed);
      set({ config: parsed });
    } catch (e: any) {
      set({ configError: 'Invalid JSON: ' + e.message });
    }
  },
  saveConfig: async () => {
    await apiPost('/api/config/save');
  },

  // Status
  status: defaultFusionStatus,
  inputStatus: defaultInputStatus,
  intercalibrationStatus: defaultIntercalibrationStatus,
  fusedPose: {},
  fusedVehiclePose: {},
  opticalData: {},
  setStatus: (s) => set({ status: s }),
  setInputStatus: (s) => set({ inputStatus: s }),
  setIntercalibrationStatus: (s) => set({ intercalibrationStatus: s }),
  setFusedPose: (p) => set({ fusedPose: p }),
  setFusedVehiclePose: (p) => set({ fusedVehiclePose: p }),
  setOpticalData: (d) => set({ opticalData: d }),

  // Rates
  inputRates: { nImu: 0, nOptical: 0, nGnss: 0, nCan: 0, nRtcmData: 0, nVehicleSpeed: 0 },
  fusionRates: { nImu: 0, nOptical: 0 },
  setInputRates: (r) => set({ inputRates: r }),
  setFusionRates: (r) => set({ fusionRates: r }),

  // Node statuses
  paused: false,
  nodeStatuses: {},
  nodeRates: {},
  setNodeStatuses: (payload) => {
    const { paused: p, nodes } = payload;
    const prev = get().nodeStatuses;
    const rates: Record<string, NodeRateEntry> = {};
    const consoleLogs = { ...get().nodeConsoleLogs };
    const MAX_PER_NODE = 500;
    for (const [key, entry] of Object.entries(nodes)) {
      const pr = prev[key];
      if (pr) {
        const di = entry.inputCount - pr.inputCount;
        const dout = entry.outputCount - pr.outputCount;
        rates[key] = {
          inputRate: di > 0 ? di : 0,
          outputRate: dout > 0 ? dout : 0,
        };
      } else {
        rates[key] = { inputRate: 0, outputRate: 0 };
      }
      if (entry.logs && entry.logs.length > 0) {
        const existing = consoleLogs[key] || [];
        const combined = existing.concat(entry.logs);
        consoleLogs[key] = combined.length > MAX_PER_NODE
          ? combined.slice(-MAX_PER_NODE) : combined;
      }
    }
    set({ paused: p, nodeStatuses: nodes, nodeRates: rates, nodeConsoleLogs: consoleLogs });
  },
  restart: async () => {
    try { await apiPost('/api/restart'); } catch {}
  },
  togglePause: async () => {
    const endpoint = get().paused ? '/api/resume' : '/api/pause';
    try {
      await apiPost(endpoint);
      set({ paused: !get().paused });
    } catch {}
  },

  // MCP
  mcpStatus: null,
  setMcpStatus: (s) => set({ mcpStatus: s }),

  aiMonitorStatus: null,
  setAiMonitorStatus: (s) => set({ aiMonitorStatus: s }),

  // Oscilloscope
  oscilloscopeData: { t: [], v: [] },
  pushOscilloscopePoint: (t, v) => set((s) => {
    const MAX_POINTS = 600;
    const td = s.oscilloscopeData;
    const newT = td.t.length >= MAX_POINTS ? [...td.t.slice(1), t] : [...td.t, t];
    const newV = td.v.length >= MAX_POINTS ? [...td.v.slice(1), v] : [...td.v, v];
    return { oscilloscopeData: { t: newT, v: newV } };
  }),
  clearOscilloscopeData: () => set({ oscilloscopeData: { t: [], v: [] } }),
  oscilloscopeTypes: [],
  setOscilloscopeTypes: (types) => set({ oscilloscopeTypes: types }),

  // Node console logs
  nodeConsoleLogs: {},
  clearNodeConsoleLogs: (nodeKey) => set((s) => {
    if (nodeKey) {
      const logs = { ...s.nodeConsoleLogs };
      delete logs[nodeKey];
      return { nodeConsoleLogs: logs };
    }
    return { nodeConsoleLogs: {} };
  }),

  // Logs
  logEntries: [],
  addLogEntries: (entries) => set((s) => {
    const combined = s.logEntries.concat(entries);
    return { logEntries: combined.length > 2000 ? combined.slice(-2000) : combined };
  }),
  clearLogEntries: () => set({ logEntries: [] }),

  // License
  license: {
    info: defaultLicenseInfo,
    method: 'file',
    licenseFile: 'license.json',
    licenseKey: '',
    serverUrl: 'http://3.114.50.38:3100/api/v1',
    loading: false,
    message: '',
    messageType: '',
    machines: [],
    machinesMax: 0,
    machinesLoading: false,
    machinesError: '',
  },
  setLicenseInfo: (info) => set((s) => ({ license: { ...s.license, info } })),
  setLicenseField: (field, value) => set((s) => ({ license: { ...s.license, [field]: value } })),
  fetchLicenseStatus: async () => {
    try {
      const data = await apiGet<any>('/api/license/status');
      if (data.license) {
        const updates: Partial<AppState['license']> = { info: data.license };
        if (data.licenseKey) updates.licenseKey = data.licenseKey;
        if (data.serverUrl) updates.serverUrl = data.serverUrl;
        set((s) => ({ license: { ...s.license, ...updates } }));
      }
    } catch {}
  },
  checkLicenseFile: async () => {
    const { license } = get();
    set((s) => ({ license: { ...s.license, loading: true, message: '' } }));
    try {
      const data = await apiPost<any>('/api/license/check-file', { licenseFile: license.licenseFile });
      set((s) => ({
        license: {
          ...s.license,
          info: data.license || s.license.info,
          message: data.license?.valid ? 'License valid' : (data.license?.error || 'License invalid'),
          messageType: data.license?.valid ? 'success' : 'error',
          loading: false,
        },
      }));
    } catch {
      set((s) => ({ license: { ...s.license, message: 'Request failed', messageType: 'error', loading: false } }));
    }
  },
  checkLicenseServer: async () => {
    const { license } = get();
    set((s) => ({ license: { ...s.license, loading: true, message: '' } }));
    try {
      const data = await apiPost<any>('/api/license/check-server', {
        licenseFile: license.licenseFile,
        licenseKey: license.licenseKey,
        serverUrl: license.serverUrl,
      });
      set((s) => ({
        license: {
          ...s.license,
          info: data.license || s.license.info,
          message: data.license?.valid ? 'License activated' : (data.license?.error || 'Activation failed'),
          messageType: data.license?.valid ? 'success' : 'error',
          loading: false,
        },
      }));
    } catch {
      set((s) => ({ license: { ...s.license, message: 'Request failed', messageType: 'error', loading: false } }));
    }
  },
  checkLicenseToken: async () => {
    set((s) => ({ license: { ...s.license, loading: true, message: '' } }));
    try {
      const data = await apiPost<any>('/api/license/check-token', {});
      set((s) => ({
        license: {
          ...s.license,
          info: data.license || s.license.info,
          message: data.license?.valid ? 'USB token valid' : (data.license?.error || 'No valid token found'),
          messageType: data.license?.valid ? 'success' : 'error',
          loading: false,
        },
      }));
    } catch {
      set((s) => ({ license: { ...s.license, message: 'Request failed', messageType: 'error', loading: false } }));
    }
  },
  uploadLicense: async (file: File) => {
    set((s) => ({ license: { ...s.license, loading: true, message: '' } }));
    try {
      const form = new FormData();
      form.append('file', file);
      const data = await apiPostFormData<any>('/api/license/upload', form);
      set((s) => ({
        license: {
          ...s.license,
          info: data.license || s.license.info,
          message: data.license?.valid ? 'License uploaded and verified' : (data.license?.error || 'Invalid license file'),
          messageType: data.license?.valid ? 'success' : 'error',
          loading: false,
        },
      }));
    } catch {
      set((s) => ({ license: { ...s.license, message: 'Upload failed', messageType: 'error', loading: false } }));
    }
  },
  fetchMachines: async () => {
    const { license } = get();
    const key = license.licenseKey || license.info.license_key;
    if (!key || !license.serverUrl) return;
    set((s) => ({ license: { ...s.license, machinesLoading: true, machinesError: '' } }));
    try {
      const data = await apiPost<any>('/api/license/machines', {
        licenseKey: key,
        serverUrl: license.serverUrl,
      });
      if (data.status === 'OK' && data.data) {
        set((s) => ({
          license: { ...s.license, machines: data.data.active_machines || [], machinesMax: data.data.max_machines || 0, machinesLoading: false },
        }));
      } else {
        set((s) => ({ license: { ...s.license, machinesError: data.error || 'Failed to load machines', machinesLoading: false } }));
      }
    } catch {
      set((s) => ({ license: { ...s.license, machinesError: 'Request failed', machinesLoading: false } }));
    }
  },
  deactivateMachine: async (machineCode: string) => {
    const { license } = get();
    const key = license.licenseKey || license.info.license_key;
    set((s) => ({ license: { ...s.license, machinesLoading: true, machinesError: '' } }));
    try {
      const data = await apiPost<any>('/api/license/deactivate-machine', {
        licenseKey: key,
        serverUrl: license.serverUrl,
        machineCode,
      });
      if (data.status === 'OK') {
        await get().fetchMachines();
      } else {
        set((s) => ({ license: { ...s.license, machinesError: data.error || 'Failed to remove machine', machinesLoading: false } }));
      }
    } catch {
      set((s) => ({ license: { ...s.license, machinesError: 'Request failed', machinesLoading: false } }));
    }
  },
}));

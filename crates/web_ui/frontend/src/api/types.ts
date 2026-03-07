export interface FusionStatus {
  autocalibrationDurationSinceLast: number;
  autocalibrationStatus: boolean;
  autocalibrationUpdated: boolean;
  autocalibrationValue: any;
  matcherStatus: {
    finished: boolean;
    nPoses: number;
    nTotalPoses: number[];
    nUsedPoses: number;
    quat: Quaternion;
  };
  nImu: number;
  nOptical: number;
  sensorMotionStatus: number;
}

export interface InputStatus {
  nImu: number;
  nOptical: number;
  nGnss: number;
  nCan: number;
  nRtcmData: number;
  nVehicleSpeed: number;
  gnssQuality: number;
}

export interface IntercalibrationStatus {
  nPoses: number;
  nTotalPoses: number[];
  nUsedPoses: number;
  quat: Quaternion | null;
  trans: Vector3 | null;
  minNPoses: number;
  finished: boolean;
  isRunning: boolean;
}

export interface Quaternion {
  w: number;
  x: number;
  y: number;
  z: number;
}

export interface Vector3 {
  x: number;
  y: number;
  z: number;
}

export interface FusedPose {
  orientation?: Quaternion;
  position?: Vector3;
  [key: string]: any;
}

export interface OpticalData {
  orientation?: Quaternion;
  position?: Vector3;
  [key: string]: any;
}

export interface FusedVehiclePose {
  globalPosition?: { x: number; y: number };
  yaw?: number;
  timestamp?: number;
  [key: string]: any;
}

export interface LicenseInfo {
  valid: boolean;
  status: string;
  customer: string;
  product: string;
  features: string[];
  expires: string | null;
  lease_expires: string | null;
  machine_code: string;
  license_key: string;
  error: string;
}

export interface InputRates {
  nImu: number;
  nOptical: number;
  nGnss: number;
  nCan: number;
  nRtcmData: number;
  nVehicleSpeed: number;
}

export interface FusionRates {
  nImu: number;
  nOptical: number;
}

export interface NodeStatusEntry {
  displayName: string;
  role: 'source' | 'filter' | 'sink';
  color: string;
  enabled: boolean;
  inputCount: number;
  outputCount: number;
  nodeStatus: Record<string, any> | null;
}

export type NodeStatuses = Record<string, NodeStatusEntry>;

export interface NodeRateEntry {
  inputRate: number;
  outputRate: number;
}

export interface NodeStatusPayload {
  paused: boolean;
  nodes: NodeStatuses;
}

export interface McpToolCallEntry {
  tool: string;
  time: string;
}

export interface McpStatus {
  connected: boolean;
  clientName: string | null;
  connectedSince: string | null;
  toolCallCount: number;
  lastTool: string | null;
  lastToolTime: string | null;
  recentTools: McpToolCallEntry[];
}

export interface LogEntry {
  ts: string;
  level: string;
  target: string;
  message: string;
}

import type { InputRates, FusionRates } from '../../../api/types';

// Maps node config keys / node type IDs to their corresponding rate fields
const INPUT_RATE_MAP: Record<string, keyof InputRates> = {
  imu: 'nImu',
  optical: 'nOptical',
  gnss: 'nGnss',
  can: 'nCan',
  rtcm: 'nRtcmData',
  vehicleSpeed: 'nVehicleSpeed',
  velocityMeter: 'nVehicleSpeed',
};

const FUSION_RATE_MAP: Record<string, keyof FusionRates> = {
  fusion: 'nImu',
  gnssImuFusion: 'nImu',
  vehicularFusion: 'nImu',
  insideOutFusion: 'nImu',
  fullFusion: 'nImu',
  fullVehicleFusion: 'nImu',
};

export type NodeStatus = 'active' | 'idle' | 'inactive';

export function getNodeRate(
  nodeTypeId: string,
  configKey: string,
  role: string,
  inputRates: InputRates,
  fusionRates: FusionRates,
): number {
  const baseKey = configKey.replace(/_\d+$/, '');

  if (role === 'source') {
    const field = INPUT_RATE_MAP[baseKey] || INPUT_RATE_MAP[nodeTypeId];
    if (field) return inputRates[field] || 0;
  }

  if (role === 'filter') {
    const field = FUSION_RATE_MAP[baseKey] || FUSION_RATE_MAP[nodeTypeId];
    if (field) return fusionRates[field] || 0;
  }

  // Sinks: active if any upstream is active (determined by edges, not here)
  return -1; // unknown
}

export function getNodeStatus(rate: number): NodeStatus {
  if (rate < 0) return 'inactive'; // unknown
  if (rate > 0) return 'active';
  return 'idle';
}

export function statusColor(status: NodeStatus): string {
  switch (status) {
    case 'active': return '#4ade80';
    case 'idle': return '#facc15';
    case 'inactive': return '#666';
  }
}

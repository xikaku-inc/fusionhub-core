export type NodeRole = 'source' | 'filter' | 'sink';

export type DataType =
  | 'Imu' | 'Gnss' | 'Optical' | 'FusedPose'
  | 'FusedVehiclePose' | 'FusedVehiclePoseV2' | 'GlobalFusedPose'
  | 'FusionStateInt' | 'Rtcm' | 'Can'
  | 'VehicleState' | 'VehicleSpeed' | 'VelocityMeter'
  | 'Any';

export interface SettingsField {
  key: string;
  label: string;
  type: 'string' | 'number' | 'boolean' | 'quaternion' | 'vector3' | 'json' | 'filepath' | 'heading';
  default: any;
}

export interface NodeSubtype {
  value: string;
  displayName: string;
  additionalSettings: SettingsField[];
}

export interface NodeTypeDefinition {
  id: string;
  displayName: string;
  role: NodeRole;
  subtypes?: NodeSubtype[];
  outputs: DataType[];
  inputs: DataType[];
  defaultSettings: Record<string, any>;
  settingsSchema: SettingsField[];
  configAliases: string[];
  color: string;
}

export interface UiExtension {
  id: string;
  displayName: string;
  route: string;
  navSection: string;
  requiredNodes?: string[];
  component?: React.ComponentType<any>;
  bundleId?: string;
  instanceKey?: string;
}

export interface EditorNode extends Record<string, unknown> {
  configKey: string;
  nodeType: NodeTypeDefinition;
  subtype?: string;
  settings: Record<string, any>;
  endpoint: string;
  disabled: boolean;
  originalConfig?: any;
  externalDirection?: 'input' | 'output';
}

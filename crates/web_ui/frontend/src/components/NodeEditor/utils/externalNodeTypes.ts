import type { NodeTypeDefinition } from '../../../types/nodes';

export const EXTERNAL_INPUT_TYPE: NodeTypeDefinition = {
  id: '_external_input',
  displayName: 'External Input',
  role: 'source',
  outputs: [],
  inputs: [],
  defaultSettings: {},
  settingsSchema: [],
  configAliases: [],
  color: '#6c8cff',
};

export const EXTERNAL_OUTPUT_TYPE: NodeTypeDefinition = {
  id: '_external_output',
  displayName: 'External Output',
  role: 'sink',
  outputs: [],
  inputs: [],
  defaultSettings: {},
  settingsSchema: [],
  configAliases: [],
  color: '#f59e0b',
};

export function sanitizeEndpointId(ep: string): string {
  return ep.replace(/[:/.*]/g, '_');
}

export function isTcpEndpoint(ep: string): boolean {
  return ep.startsWith('tcp://');
}

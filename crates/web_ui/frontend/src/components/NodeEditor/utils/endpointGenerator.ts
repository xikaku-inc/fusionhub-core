let counter = 0;

export function generateEndpoint(nodeTypeId: string, nodeId?: string): string {
  counter++;
  const id = nodeId || `${counter}`;
  return `inproc://${nodeTypeId}_data_${id}`;
}

export function resetEndpointCounter(): void {
  counter = 0;
}

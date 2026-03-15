import type { Connection, Node, Edge } from '@xyflow/react';
import type { EditorNode } from '../../../types/nodes';

export function isValidConnection(
  connection: Connection,
  nodes: Node[],
  edges: Edge[],
): boolean {
  if (!connection.source || !connection.target) return false;
  if (connection.source === connection.target) return false;

  // Prevent duplicate connections
  const exists = edges.some(
    (e) =>
      e.source === connection.source &&
      e.target === connection.target &&
      e.sourceHandle === connection.sourceHandle &&
      e.targetHandle === connection.targetHandle,
  );
  if (exists) return false;

  const sourceNode = nodes.find((n) => n.id === connection.source);
  const targetNode = nodes.find((n) => n.id === connection.target);
  if (!sourceNode || !targetNode) return false;

  const sourceData = sourceNode.data as EditorNode;
  const targetData = targetNode.data as EditorNode;

  // Extract data type from handle IDs
  const outputType = connection.sourceHandle?.replace('out-', '');
  const inputType = connection.targetHandle?.replace('in-', '');
  const outLower = outputType?.toLowerCase();
  const inLower = inputType?.toLowerCase();

  // "any"/"Any" handles (including numbered any-0, any-1, ...) accept/produce everything
  if (inLower?.startsWith('any') || outLower?.startsWith('any')) return true;

  // ext handles are universal connectors (external TCP endpoints)
  if (outputType === 'ext' || inputType === 'ext') return true;

  // Dynamic output connects to any input
  if (outputType === 'Dynamic') return true;

  // Nodes with no specific inputs accept any
  if (targetData.nodeType.inputs.length === 0) {
    return true;
  }

  // Check if the output data type matches the input data type
  if (outputType && inputType) {
    return outputType === inputType;
  }

  return false;
}

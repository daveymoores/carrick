import type { WidgetDto } from '@acme/models';

export interface WidgetResponse {
  widget: WidgetDto;
  fetchedAt: string;
}

export async function listWidgets(): Promise<WidgetDto[]> {
  return [];
}

export function run(
  publish: (topic: string, payload: unknown) => void,
  w: WidgetDto
): void {
  publish('widget.updated', { widget: w, at: 'now' });
}

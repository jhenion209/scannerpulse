// The admin page's areas, in the side menu's order, and the one a link names
// (`/admin#maintenance`); the first is the default.

export const AREAS = [
  { id: 'status', label: 'Status' },
  { id: 'maintenance', label: 'Maintenance' },
] as const;

export type AreaId = (typeof AREAS)[number]['id'];

export const areaOf = (hash: string): AreaId => AREAS.find((a) => `#${a.id}` === hash)?.id ?? AREAS[0].id;

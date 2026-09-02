import type { ReactNode, SVGProps } from 'react'

export type AppIconName =
  | 'bell'
  | 'chevron-down'
  | 'chevron-right'
  | 'circle-minus'
  | 'columns-3'
  | 'copy-plus'
  | 'ellipsis'
  | 'folder'
  | 'folder-input'
  | 'git-branch'
  | 'git-commit-horizontal'
  | 'graph-ring'
  | 'layers'
  | 'layers-plus'
  | 'network'
  | 'panel-right-open'
  | 'pencil'
  | 'pin'
  | 'plus'
  | 'search'
  | 'settings-2'
  | 'square-pen'
  | 'trash-2'
  | 'upload'
  | 'x'

export function AppIcon({ name, size = 16, ...props }: {
  name: AppIconName
  size?: number
} & Omit<SVGProps<SVGSVGElement>, 'children' | 'name'>) {
  return <svg {...props} data-icon={name} aria-hidden="true" focusable="false"
    width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
    {paths[name]}
  </svg>
}

const paths: Record<AppIconName, ReactNode> = {
  'square-pen': <><path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="m17.6 3.6 2.8 2.8L11 15.8l-3.5.7.7-3.5Z"/></>,
  plus: <><path d="M12 5v14"/><path d="M5 12h14"/></>,
  'graph-ring': <><path d="M13.8 5.6a8.2 8.2 0 0 1 4.5 7.8M16.5 17.5a8.2 8.2 0 0 1-9 0M5.7 13.4a8.2 8.2 0 0 1 4.5-7.8"/><circle cx="12" cy="4.5" r="2.15" fill="currentColor" stroke="none"/><circle cx="18.5" cy="16" r="2.15" fill="currentColor" stroke="none"/><circle cx="5.5" cy="16" r="2.15" fill="currentColor" stroke="none"/></>,
  'panel-right-open': <><rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M14 4v16M17.5 9v6M20.5 12h-6"/></>,
  'folder-input': <><path d="M3 7.5V6a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M12 9v7m-3-3 3 3 3-3"/></>,
  layers: <><path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/></>,
  'layers-plus': <><path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 .83.18a2 2 0 0 0 .83-.18l8.58-3.9a1 1 0 0 0 0-1.831zM16 17h6m-3-3v6M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 .825.178M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l2.116-.962"/></>,
  'copy-plus': <><path d="M15 12v6m-3-3h6"/><rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></>,
  'circle-minus': <><circle cx="12" cy="12" r="10"/><path d="M8 12h8"/></>,
  ellipsis: <><circle cx="5" cy="12" r="1.25" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.25" fill="currentColor" stroke="none"/><circle cx="19" cy="12" r="1.25" fill="currentColor" stroke="none"/></>,
  x: <><path d="m7 7 10 10"/><path d="M17 7 7 17"/></>,
  bell: <><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9"/><path d="M10 21h4"/></>,
  folder: <path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>,
  pin: <><path d="m15 4 5 5-3 1-4 4 1 4-1 1-4-4-4 4-1-1 4-4-4-4 1-1 4 1 4-4Z"/><path d="m4 20 5-5"/></>,
  'columns-3': <><rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9 4v16M15 4v16"/></>,
  'settings-2': <><path d="M4 7h7M15 7h5M4 17h3M11 17h9"/><circle cx="13" cy="7" r="2"/><circle cx="9" cy="17" r="2"/></>,
  'git-branch': <><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M8 11h3a5 5 0 0 0 5-5"/></>,
  network: <><rect x="9" y="3" width="6" height="5" rx="1.5"/><rect x="3" y="16" width="6" height="5" rx="1.5"/><rect x="15" y="16" width="6" height="5" rx="1.5"/><path d="M12 8v4M6 16v-2a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v2"/></>,
  'git-commit-horizontal': <><path d="M3 12h5M16 12h5"/><circle cx="12" cy="12" r="4"/></>,
  upload: <><path d="M12 16V4m-4 4 4-4 4 4"/><path d="M5 14v4a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-4"/></>,
  search: <><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4.5 4.5"/></>,
  'chevron-down': <path d="m7 10 5 5 5-5"/>,
  'chevron-right': <path d="m10 7 5 5-5 5"/>,
  'trash-2': <><path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5M14 11v5"/></>,
  pencil: <><path d="m15 5 4 4L8 20l-5 1 1-5Z"/><path d="m13 7 4 4"/></>
}

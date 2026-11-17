// Key bindings for the file list. Pure, so the table is testable without a DOM.

export type ListIntent =
  | { kind: 'move'; to: 'next' | 'previous' | 'first' | 'last'; extend: boolean }
  | { kind: 'open' }
  | { kind: 'rename' }
  | { kind: 'trash' }
  | { kind: 'toggle' }
  | { kind: 'clear' }
  | { kind: 'search' }
  | { kind: 'help' };

export type KeyInput = Pick<KeyboardEvent, 'key' | 'shiftKey' | 'ctrlKey' | 'metaKey' | 'altKey' | 'defaultPrevented' | 'isComposing'>;

// Listed in the shortcut help; keep in step with listIntent.
export const shortcutHelp: ReadonlyArray<{ keys: string; action: string }> = [
  { keys: 'Up, Down', action: 'Move between items' },
  { keys: 'Home, End', action: 'First or last item' },
  { keys: 'Shift+Up, Shift+Down', action: 'Extend the selection' },
  { keys: 'X', action: 'Select or clear the focused item' },
  { keys: 'Escape', action: 'Clear the selection' },
  { keys: 'Enter', action: 'Open the focused item' },
  { keys: 'F2', action: 'Rename the focused item' },
  { keys: 'Delete, Backspace', action: 'Move to Trash, after confirmation' },
  { keys: '/', action: 'Search (in the list)' },
  { keys: '?', action: 'Show these shortcuts (in the list)' },
];

const moves: Record<string, 'next' | 'previous' | 'first' | 'last'> = { ArrowDown: 'next', ArrowUp: 'previous', Home: 'first', End: 'last' };

/**
 * Maps a key press to an intent. `onItem` is true when focus is on a list row's name or checkbox.
 * `inList` is true when focus is anywhere in the list (a row or its controls, or the folder heading).
 * Every single-character key needs `inList`, so none fire elsewhere (WCAG 2.1.4, active only on focus).
 */
export function listIntent(event: KeyInput, onItem: boolean, inList: boolean): ListIntent | null {
  if (event.defaultPrevented || event.isComposing || event.ctrlKey || event.metaKey || event.altKey) return null;
  if (!inList) return null;
  if (event.key === '/' && !event.shiftKey) return { kind: 'search' };
  if (event.key === '?') return { kind: 'help' };
  if (!onItem) return null;
  const to = moves[event.key];
  if (to) return { kind: 'move', to, extend: event.shiftKey && (to === 'next' || to === 'previous') };
  if (event.shiftKey) return null;
  switch (event.key) {
    case 'Enter': return { kind: 'open' };
    case 'F2': return { kind: 'rename' };
    case 'Delete': case 'Backspace': return { kind: 'trash' };
    case 'x': case 'X': return { kind: 'toggle' };
    case 'Escape': return { kind: 'clear' };
    default: return null;
  }
}

/** True when a shortcut must not fire: typing, or a dialog or menu owns the keyboard. */
export function shortcutsBlocked(target: EventTarget | null, doc: Document): boolean {
  if (target instanceof HTMLElement) {
    if (target.isContentEditable || target.matches('textarea, select')) return true;
    if (target instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit', 'reset'].includes(target.type)) return true;
  }
  return !!doc.querySelector('[role=dialog][data-open], [role=alertdialog][data-open], [role=menu]');
}

/** Index of the row to focus after a move, or -1 when there are no rows. */
export function moveIndex(current: number, count: number, to: 'next' | 'previous' | 'first' | 'last'): number {
  if (!count) return -1;
  if (to === 'first') return 0;
  if (to === 'last') return count - 1;
  if (current < 0) return to === 'next' ? 0 : count - 1;
  return Math.min(count - 1, Math.max(0, current + (to === 'next' ? 1 : -1)));
}

/** Entries an action applies to: the selection when the focused one is in it, else just the focused one. */
export function actionTargets<T extends { id: string }>(focusedId: string, selected: readonly T[], all: readonly T[]): T[] {
  if (selected.some((entry) => entry.id === focusedId)) return [...selected];
  const focused = all.find((entry) => entry.id === focusedId);
  return focused ? [focused] : [];
}

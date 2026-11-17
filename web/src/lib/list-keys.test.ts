import assert from 'node:assert/strict';
import { test } from 'node:test';
import { actionTargets, listIntent, moveIndex, type KeyInput } from './list-keys.ts';

const key = (k: string, extra: Partial<KeyInput> = {}): KeyInput => ({ key: k, shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, defaultPrevented: false, isComposing: false, ...extra });

test('list keys only act on an item, except search and help in the list', () => {
  assert.equal(listIntent(key('F2'), false, true), null);
  assert.deepEqual(listIntent(key('F2'), true, true), { kind: 'rename' });
  assert.deepEqual(listIntent(key('/'), false, true), { kind: 'search' });
  assert.deepEqual(listIntent(key('?', { shiftKey: true }), false, true), { kind: 'help' });
});

test('character keys do nothing when focus is outside the list', () => {
  for (const k of ['/', '?', 'x', 'X']) assert.equal(listIntent(key(k), false, false), null);
  assert.equal(listIntent(key('?', { shiftKey: true }), false, false), null);
});

test('shift extends only with up and down', () => {
  assert.deepEqual(listIntent(key('ArrowDown', { shiftKey: true }), true, true), { kind: 'move', to: 'next', extend: true });
  assert.deepEqual(listIntent(key('End', { shiftKey: true }), true, true), { kind: 'move', to: 'last', extend: false });
  assert.equal(listIntent(key('Delete', { shiftKey: true }), true, true), null);
});

test('browser and screen-reader chords are left alone', () => {
  for (const mod of ['ctrlKey', 'metaKey', 'altKey'] as const) {
    assert.equal(listIntent(key('ArrowDown', { [mod]: true }), true, true), null);
    assert.equal(listIntent(key('/', { [mod]: true }), false, true), null);
  }
  assert.equal(listIntent(key('Delete', { isComposing: true }), true, true), null);
  assert.equal(listIntent(key('Delete', { defaultPrevented: true }), true, true), null);
});

test('moveIndex clamps at the ends', () => {
  assert.equal(moveIndex(0, 3, 'previous'), 0);
  assert.equal(moveIndex(2, 3, 'next'), 2);
  assert.equal(moveIndex(-1, 3, 'next'), 0);
  assert.equal(moveIndex(1, 0, 'next'), -1);
});

test('actions apply to the selection only when the focused item is in it', () => {
  const all = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
  assert.deepEqual(actionTargets('a', [all[0], all[1]], all), [all[0], all[1]]);
  assert.deepEqual(actionTargets('c', [all[0], all[1]], all), [all[2]]);
  assert.deepEqual(actionTargets('z', [], all), []);
});

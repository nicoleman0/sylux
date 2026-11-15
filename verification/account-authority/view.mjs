export const title = 'Two admins try to remove each other';
const accounts = ['A', 'B'];
const admins = v => accounts.filter(a => v.enabled[a] && v.role[a] === 'administrator');
export function state(v) {
  const left = admins(v);
  if (!left.length) return 'No enabled admin remains';
  const done = accounts.filter(a => v.stage[a] === 'done');
  if (done.length === 2) return `Admin ${left[0]} remains; other request refused`;
  if (done.length === 1) return `Admin ${left[0]} remains; other request waits`;
  const checked = accounts.filter(a => v.stage[a] === 'checked');
  if (checked.length === 2) return 'Both requests hold permission from the old state';
  if (checked.length === 1) return `Admin ${checked[0]} holds permission from the old state`;
  return 'Two enabled admins; both requests wait';
}
export function step(action, from, to) {
  if (action === 'Done') return 'both requests finished';
  const checked = accounts.find(a => from.stage[a] === 'pending' && to.stage[a] === 'checked');
  if (checked) return `admin ${checked} caches permission and admin count`;
  const actor = accounts.find(a => from.stage[a] !== 'done' && to.stage[a] === 'done');
  if (!actor) return action;
  if (to.result[actor] !== 204) return `admin ${actor} request refused [session revoked]`;
  const target = actor === 'A' ? 'B' : 'A';
  return `admin ${actor} ${to.kind[actor] === 'disable' ? 'disables' : 'demotes'} admin ${target}`;
}
export const group = v => !admins(v).length ? 'broken' : 'safe';
export const colors = { safe: 'teal', broken: 'pink' };

# Account actions review

SYL-66. Inspected 15 November 2026 before implementation.

## Comparison

- [Nextcloud 35 user management](https://docs.nextcloud.com/server/stable/admin_manual/configuration_user/user_configuration.html), including its [actions screenshot](https://docs.nextcloud.com/server/stable/admin_manual/_images/user-config-actions.png): inspected the rendered screenshot. A compact ellipsis menu groups actions beside each row. Adopt grouping and reversible disabling; reject hover-only discovery and deletion. Sylux keeps a visible, named trigger on every row and its existing consequence dialogs.
- [File Browser Users.vue, upstream master](https://github.com/filebrowser/filebrowser/blob/master/frontend/src/views/settings/Users.vue), unversioned source inspected on the date above: a username/admin/scope table has a persistent edit link per row. Adopt a persistent row action and compact role column. Reject a separate full editing page for these few existing actions. No live instance or screenshot was available for this comparison; source inspection establishes structure, not rendered appearance.

Neither comparison was a live competitor test.

## Existing behavior and decisions

`src/http/accounts.rs` already supports enable, disable, role change and standard-user password reset. Authority changes reject self disable/demotion and removing the last enabled administrator, then revoke the target's sessions in the same transaction. Password reset is restricted to other enabled standard users. No backend changes are needed.

The menu preserves that action eligibility. A self row shows You and a route to personal account settings, with a reason for unavailable authority changes. The last enabled administrator gets an explicit explanation. Other rows retain existing confirmations and error recovery. Dialog cancellation returns focus to the persistent row trigger. Active uses a green circle/check; disabled uses a red square/cross. Both have accessible names and keyboard-accessible tooltips. Existing primary/destructive semantic colors provide theme-specific contrast without adding tokens.

Accounts keep their four columns at narrow widths. The action target remains 44px. The catalogue includes both statuses and self/standard-user menus. Account deletion and new account APIs are excluded.

## Verification

- Final Svelte check: zero errors or warnings. Production build passed.
- Five focused account-menu tests pass: 1280px/390px in light/dark, named status tooltips, keyboard menu dismissal/focus, self settings route, duplicate prevention, pending Escape, uncertain-result recovery and explicit refresh feedback clearing. Screenshots cover each viewport/theme and open menus.
- The integrated Chromium run passed 133 tests across account actions, search responsiveness, smoke, browsing context, column sorting and the design catalogue. The opt-in search timing experiment was the only skipped test. Account coverage includes create/reset/disable, promotion/demotion/enable, conflict/not-found/server-error/interrupted outcomes, cancellation focus, session changes and layout.
- New pending verification caught an existing dialog boundary issue: block Escape/outside events while a request is active, rather than relying only on the open-state callback. Explicit Accounts refresh now clears completed account feedback; automatic post-operation reload still preserves its acknowledgement.
- Icon-to-workspace contrast from the existing token values: active 6.28:1 light / 9.71:1 dark, disabled 7.14:1 light / 8.49:1 dark. Both exceed 3:1.
- Six real HTTP account-authority contracts passed, including the concurrent race. The bounded model positive configuration and negative control passed. No backend authority sources or model mappings changed.

Browser fixtures use intercepted disposable accounts, not a deployed instance. Native touch hardware and screen-reader speech were not exercised. No release, deployment or durability qualification is claimed.

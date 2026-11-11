import { createHash } from 'node:crypto';

const ALLOWED_CASES = new Set([
  'trusted_assets', 'trusted_api_routes', 'wrong_san', 'unrelated_ca', 'plaintext_and_cookie_policy',
  'accessibility_keyboard_sign_in', 'accessibility_control_names', 'accessibility_rename_escape_focus', 'accessibility_live_failure_retry',
  'storage_responsive_controls', 'storage_listing_states', 'storage_upload_download_roundtrip', 'storage_delayed_upload_browsing',
  'storage_upload_cancel', 'storage_lost_commit_response',
]);
const ALLOWED_LISTING_PROGRESS = new Set([
  'signed-in', 'first-folder-response-held', 'returned-to-root', 'nested-file-visible',
  'breadcrumb-roundtrip-complete', 'error-state-complete', 'empty-folder-complete',
  'race-parent-open', 'stale-response-held', 'nested-folder-open-under-race',
  'stale-response-drained', 'first-page-visible', 'next-page-visible', 'previous-page-verified',
]);

// Allowlist-only: never persist titles, stdout/stderr, errors, headers, request
// bodies, attachments, trace archives or storage state. A failing assertion can
// contain a password even when screenshots/traces are disabled.
export default class SafeReporter {
  onBegin(_config: unknown, suite: { allTests(): { annotations: { type: string; description?: string }[] }[] }) {
    const cases = suite.allTests().map(t => t.annotations.find(a => a.type === 'harness_case')?.description)
      .map(id => id && ALLOWED_CASES.has(id) ? id : 'unregistered');
    process.stdout.write(JSON.stringify({ event: 'discovery', cases }) + '\n');
  }
  onTestEnd(test: { id: string; annotations?: { type: string; description?: string }[] }, result: { status: string; errors?: { stack?: string; message?: string }[] }) {
    // Numeric source location only; never persist assertion values or error text.
    const stack = result.errors?.[0]?.stack ?? '';
    const location = /\b(harness|storage|accessibility)\.spec\.ts:(\d+):\d+/.exec(stack);
    const failureLine = location ? Number(location[2]) : 0;
    const message = result.errors?.[0]?.message ?? '';
    const network = /net::(ERR_[A-Z_]+)/.exec(message + stack);
    const resolved = (message + stack).includes('Received promise resolved instead of rejected');
    const caseId = test.annotations?.find(a => a.type === 'harness_case')?.description;
    const listingProgress = (test.annotations ?? [])
      .filter(a => a.type === 's1_listing_progress' && a.description && ALLOWED_LISTING_PROGRESS.has(a.description))
      .map(a => a.description);
    const statuses = ['passed', 'failed', 'timedOut', 'skipped', 'interrupted'];
    process.stdout.write(JSON.stringify({
      event: 'case',
      id: createHash('sha256').update(test.id).digest('hex'),
      case_id: caseId && ALLOWED_CASES.has(caseId) ? caseId : 'unregistered',
      status: statuses.includes(result.status) ? result.status : 'unknown',
      ...(listingProgress.length ? { listing_progress: listingProgress } : {}),
      ...(location && Number.isSafeInteger(failureLine) && failureLine > 0 && failureLine <= 1_000_000
        ? { failure_file: `${location[1]}.spec.ts`, failure_line: failureLine } : {}),
      ...(network && ['ERR_EMPTY_RESPONSE', 'ERR_CONNECTION_RESET', 'ERR_CONNECTION_CLOSED', 'ERR_SSL_PROTOCOL_ERROR', 'ERR_CERT_AUTHORITY_INVALID', 'ERR_CERT_COMMON_NAME_INVALID', 'ERR_HTTP_RESPONSE_CODE_FAILURE', 'ERR_BLOCKED_BY_CLIENT', 'ERR_INVALID_HTTP_RESPONSE', 'ERR_UNSAFE_PORT', 'ERR_TIMED_OUT'].includes(network[1]) ? { network_error: network[1] } : {}),
      ...(resolved ? { unexpected_resolution: true } : {}),
    }) + '\n');
  }
  onStdOut() {}
  onStdErr() {}
  onError() {
    process.stdout.write('{"event":"runner_error"}\n');
  }
  onEnd(result: { status: string }) {
    const statuses = ['passed', 'failed', 'timedout', 'interrupted'];
    process.stdout.write(JSON.stringify({ event: 'end', status: statuses.includes(result.status) ? result.status : 'unknown' }) + '\n');
  }
}

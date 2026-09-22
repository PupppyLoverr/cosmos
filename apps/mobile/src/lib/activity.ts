// Pure view helpers ported from crates/proto/src/view.rs so the phone and
// the desktop describe the same sessions and the same tool groups.

import type { ToolCall } from './protocol';

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * Compact relative time ("now", "5m", "3h", "2d", "1w", "4w", "1mo", "1y").
 * Port of `format_time_ago`. No "ago" suffix.
 */
export function formatTimeAgo(iso?: string | null, now = Date.now()): string {
  if (!iso) return '';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const s = Math.max(0, Math.floor((now - then) / 1000));
  if (s < 60) return 'now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d`;
  const w = Math.floor(d / 7);
  if (w < 5) return `${w}w`;
  const mo = Math.floor(d / 30);
  if (mo < 12) return `${mo}mo`;
  return `${Math.floor(d / 365)}y`;
}

/** Desktop `tool_group_summary`: "Ran 3 commands · edited 2 files · 1 failed". */
export function toolGroupSummary(tools: { call: ToolCall; isError?: boolean }[]): string {
  let commands = 0;
  const edited: string[] = [];
  let reads = 0;
  let searches = 0;
  let fetches = 0;
  let todos = 0;
  let other = 0;
  let failed = 0;
  for (const tool of tools) {
    if (tool.isError) failed += 1;
    const call = tool.call;
    switch (call.kind) {
      case 'exec':
        commands += 1;
        break;
      case 'writeFile':
      case 'editFile':
        if (!edited.includes(call.path)) edited.push(call.path);
        break;
      case 'applyPatch': {
        const path = call.path ?? 'patch';
        if (!edited.includes(path)) edited.push(path);
        break;
      }
      case 'readFile':
        reads += 1;
        break;
      case 'search':
      case 'glob':
      case 'webSearch':
        searches += 1;
        break;
      case 'webFetch':
        fetches += 1;
        break;
      case 'todo':
        todos += 1;
        break;
      default:
        other += 1;
        break;
    }
  }
  const segments: string[] = [];
  if (commands > 0) segments.push(`ran ${plural(commands, 'command', 'commands')}`);
  if (edited.length) segments.push(`edited ${plural(edited.length, 'file', 'files')}`);
  if (reads > 0) segments.push(`read ${plural(reads, 'file', 'files')}`);
  if (searches > 0) segments.push(`searched ${plural(searches, 'time', 'times')}`);
  if (fetches > 0) segments.push(`fetched ${plural(fetches, 'page', 'pages')}`);
  if (todos > 0) segments.push('updated todos');
  if (other > 0) segments.push(`called ${plural(other, 'tool', 'tools')}`);
  if (!segments.length) segments.push(plural(tools.length, 'tool', 'tools'));
  if (failed > 0) segments.push(`${failed} failed`);
  const summary = segments.join(' · ');
  return summary.charAt(0).toUpperCase() + summary.slice(1);
}

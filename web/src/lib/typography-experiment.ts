export type TypographyExperiment = 'baseline' | 'metadata' | 'headings' | 'filenames';

const modes = new Set<TypographyExperiment>(['metadata', 'headings', 'filenames']);

export function readTypographyExperiment(search: string): TypographyExperiment {
  const value = new URLSearchParams(search).get('typography');
  return value && modes.has(value as TypographyExperiment) ? value as TypographyExperiment : 'baseline';
}

export function initTypographyExperiment(): TypographyExperiment {
  if (!import.meta.env.DEV) return 'baseline';

  const mode = readTypographyExperiment(window.location.search);
  if (mode === 'baseline') {
    delete document.documentElement.dataset.typographyExperiment;
  } else {
    document.documentElement.dataset.typographyExperiment = mode;
    void import('./typography-experiment.css');
  }
  return mode;
}

export function typographyExperimentHref(mode: TypographyExperiment, pathname = window.location.pathname): string {
  const url = new URL(window.location.href);
  url.pathname = pathname;
  if (mode === 'baseline') url.searchParams.delete('typography');
  else url.searchParams.set('typography', mode);
  return `${url.pathname}${url.search}${url.hash}`;
}

import { expect, type Page } from '@playwright/test';

export function folderSortHeader(page: Page, column: 'Name' | 'Modified' | 'Size') {
  return page.getByRole('table', { name: 'Files in current folder' }).getByRole('columnheader', { name: column, exact: true });
}

export async function chooseFolderSort(page: Page, column: 'Name' | 'Modified' | 'Size', direction: 'ascending' | 'descending') {
  const header = folderSortHeader(page, column);
  const button = header.getByRole('button');
  await expect(button).toHaveAttribute('aria-busy', 'false');
  for (let attempt = 0; attempt < 2 && await header.getAttribute('aria-sort') !== direction; attempt++) {
    const next = (await button.getAttribute('aria-label'))?.endsWith('descending') ? 'descending' : 'ascending';
    await button.click();
    await expect(header).toHaveAttribute('aria-sort', next);
  }
  await expect(header).toHaveAttribute('aria-sort', direction);
}

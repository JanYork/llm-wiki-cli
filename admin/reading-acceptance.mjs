// Run with an authenticated CUA/Playwright page and a real, non-first article.
// This only navigates the UI; it does not create or modify team data.
export async function verifyReadingResume(page, title, category) {
  await page.playwright.getByRole('button', { name: '记忆', exact: true }).press('Enter');
  await page.playwright.getByRole('button', { name: `${title} ${category}`, exact: true }).press('Enter');
  await page.playwright.getByRole('heading', { name: title, exact: true }).waitFor({ state: 'visible' });
  await page.playwright.getByRole('button', { name: '总览', exact: true }).press('Enter');
  await page.playwright.getByRole('button', { name: '记忆', exact: true }).press('Enter');
  await page.playwright.getByRole('heading', { name: title, exact: true }).waitFor({ state: 'visible' });
  await page.reload();
  await page.playwright.getByRole('heading', { name: title, exact: true }).waitFor({ state: 'visible' });
}

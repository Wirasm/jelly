// Run from the repository root:
// python3 -m http.server 7889 --bind 127.0.0.1
// playwright-cli -s=jelly-comment-placement open
// playwright-cli -s=jelly-comment-placement run-code --filename=tests/comment-placement.js --raw
// playwright-cli -s=jelly-comment-placement close
async (page) => {
  const assert = (ok, message) => { if (!ok) throw Error(message); };
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto('http://127.0.0.1:7889/tests/comment-placement.html?agent=1');
  const save = page.locator('#save');
  const comment = page.getByRole('button', { name: 'Comment', exact: true });
  await comment.waitFor({ state: 'visible' });
  const fallback = await comment.boundingBox();
  assert(Math.abs(900 - fallback.x - fallback.width - 10) < 1, 'default Comment is 10px from the right');
  assert(Math.abs(600 - fallback.y - fallback.height - 10) < 1, 'default Comment is 10px from the bottom');
  await comment.click();
  const active = page.getByRole('button', { name: 'Click a part to comment · Esc', exact: true });
  assert(await active.isVisible(), 'default Comment activates in one click');
  await page.keyboard.press('Escape');

  await page.evaluate(() => document.documentElement.classList.add('raised-comment'));
  const raised = await comment.boundingBox();
  const control = await save.boundingBox();
  assert(raised.y + raised.height <= control.y, 'Comment clears Save');
  assert(Math.abs(600 - raised.y - raised.height - 78) < 1, 'root inset sets bottom offset');
  await save.click();
  assert(await save.getAttribute('data-clicks') === '1', 'Save receives its click');
  await comment.click();
  assert(await active.isVisible(), 'Comment activates in one click');
  const expanded = await active.boundingBox();
  assert(expanded.y + expanded.height <= control.y, 'active Comment still clears Save');
  await page.keyboard.press('Escape');

  await page.evaluate(() => document.body.classList.add('top-comment'));
  const top = await comment.boundingBox();
  assert(Math.abs(top.y - 10) < 1 && top.y < raised.y, 'body class moves Comment to the top without reload');
  assert(Math.abs(top.height - raised.height) < 1, 'top placement keeps natural height');
  await comment.click();
  assert(await active.isVisible(), 'moved Comment remains clickable');
  await page.keyboard.press('Escape');
  await save.click();
  assert(await save.getAttribute('data-clicks') === '2', 'Save remains clickable after placement changes');

  await page.evaluate(() => document.body.classList.remove('top-comment'));
  const restored = await comment.boundingBox();
  assert(Math.abs(restored.y - raised.y) < 1, 'removing the class restores inherited placement');
  assert(await save.getAttribute('data-clicks') === '2', 'fixture was not reloaded');
  return { passed: true, fallback, raised, top, restored };
}

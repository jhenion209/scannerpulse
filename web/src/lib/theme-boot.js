// Runs in the page's <head>, before anything is drawn: the listener's
// chosen theme, so that the page never flashes the other one.
try {
  const theme = localStorage.getItem('scannerpulse.theme');
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch {
  // Storage may be refused: the device's preference then decides.
}

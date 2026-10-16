import type { LiveEvent } from './live';

/** Follows `/api/live`, opening it again after a drop (1 s, doubling to
 * 30 s); `url` is asked each time, so a reconnect says the newest call seen.
 * Returns the stop. */
export function follow(url: () => string, onEvent: (event: LiveEvent) => void): () => void {
  let socket: WebSocket | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let delay = 1000;
  let stopped = false;

  const open = () => {
    socket = new WebSocket(url());
    socket.onopen = () => (delay = 1000);
    socket.onmessage = (message) => {
      try {
        onEvent(JSON.parse(message.data) as LiveEvent);
      } catch {
        onEvent({ type: 'resync' });
      }
    };
    socket.onclose = () => {
      if (stopped) return;
      timer = setTimeout(open, delay);
      delay = Math.min(delay * 2, 30_000);
    };
  };
  open();
  return () => {
    stopped = true;
    clearTimeout(timer);
    socket?.close();
  };
}

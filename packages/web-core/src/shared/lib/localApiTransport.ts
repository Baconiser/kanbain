function toAbsoluteWsUrl(pathOrUrl: string): string {
  if (/^wss?:\/\//i.test(pathOrUrl)) return pathOrUrl;
  if (/^https?:\/\//i.test(pathOrUrl)) return pathOrUrl.replace(/^http/i, 'ws');

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const path = pathOrUrl.startsWith('/') ? pathOrUrl : `/${pathOrUrl}`;
  return `${protocol}//${window.location.host}${path}`;
}

export async function makeLocalApiRequest(
  pathOrUrl: string,
  init: RequestInit = {}
): Promise<Response> {
  return fetch(pathOrUrl, init);
}

export async function openLocalApiWebSocket(
  pathOrUrl: string
): Promise<WebSocket> {
  return new WebSocket(toAbsoluteWsUrl(pathOrUrl));
}

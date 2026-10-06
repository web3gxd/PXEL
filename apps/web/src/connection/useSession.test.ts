import { describe, expect, it } from 'vitest';
import { sessionEndpoints } from './useSession';
describe('session routing', () => {
  it('keeps the existing member routes and authenticates guest reconnects without dev login', () => {
    expect(sessionEndpoints()).toEqual({
      session: '/api/dev/session',
      method: 'POST',
      websocket: '/api/ws',
    });
    expect(sessionEndpoints({ linkId: 'example' })).toEqual({
      session: '/api/shared/example/session',
      method: 'GET',
      websocket: '/api/shared/example/ws',
    });
  });
});

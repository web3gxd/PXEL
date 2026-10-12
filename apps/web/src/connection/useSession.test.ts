import { describe, expect, it } from 'vitest';
import { sessionEndpoints } from './useSession';
describe('session routing', () => {
  it('uses member authentication and keeps guest reconnects separate', () => {
    expect(sessionEndpoints()).toEqual({
      session: '/api/auth/session',
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

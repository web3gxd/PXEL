import { afterEach, expect, it, vi } from 'vitest';
import {
  clipboardHtml,
  decodeInternalClipboard,
  writeSelectionClipboard,
  writeTextClipboard,
} from './outbound';
import { DocumentStore } from '../document/store';
afterEach(() => vi.unstubAllGlobals());
it('preserves Unicode and internal formats through a safe HTML attribute', () => {
  const payload = {
    schema_version: 1 as const,
    formula: JSON.stringify({ source: '=IF(A1,"я < > &",1)' }),
    tsv: '"Я😀"',
  };
  const html = clipboardHtml('Я😀', payload);
  expect(decodeInternalClipboard(JSON.stringify(payload))).toEqual(payload);
  expect(html).not.toContain('<script');
  expect(decodeInternalClipboard('{}')).toBeUndefined();
});
it('does not serialize data or create a cut intent when audit fails', async () => {
  const store = new DocumentStore();
  vi.spyOn(store, 'authorizeClipboard').mockRejectedValue(new Error('Аудит недоступен'));
  const serialize = vi.spyOn(store, 'copySelectionTsv'),
    cut = vi.spyOn(store, 'cutRangePayload');
  vi.stubGlobal(
    'ClipboardItem',
    class {
      constructor(readonly data: Record<string, Promise<Blob>>) {}
    },
  );
  vi.stubGlobal('navigator', {
    clipboard: {
      write: async (items: { data: Record<string, Promise<Blob>> }[]) => {
        await items[0]!.data['text/plain'];
      },
    },
  });
  await writeSelectionClipboard(store, 'cut');
  expect(serialize).not.toHaveBeenCalled();
  expect(cut).not.toHaveBeenCalled();
  expect(store.error).toContain('Аудит');
  store.dispose();
});
it.each([true, 'unmapped'] as const)(
  'blocks protected view %s when only an anonymized projection can be issued',
  async (view) => {
    const store = new DocumentStore();
    const authorize = vi
      .spyOn(store, 'authorizeClipboard')
      .mockResolvedValue({ anonymized: true, text: '16' });
    const afterCut = vi.fn();
    let clipboard = 'Keep clipboard';
    vi.stubGlobal(
      'ClipboardItem',
      class {
        constructor(readonly data: Record<string, Promise<Blob>>) {}
      },
    );
    vi.stubGlobal('navigator', {
      clipboard: {
        write: async (items: { data: Record<string, Promise<Blob>> }[]) => {
          const blob = await items[0]!.data['text/plain'];
          clipboard = await blob!.text();
        },
      },
    });
    await writeTextClipboard(store, '16', 'cut', afterCut, view);
    expect(authorize).toHaveBeenCalledOnce();
    expect(clipboard).toBe('Keep clipboard');
    expect(afterCut).not.toHaveBeenCalled();
    expect(store.error).toContain('обезличенная копия');
    store.dispose();
  },
);

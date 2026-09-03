import { describe, it, expect } from 'vitest';

import './widget-icons.js';

const { iconHtml } = window.widgetIcons;

describe('iconHtml — name validation', () => {
  it('maps a known icon name to its bundled file', () => {
    expect(iconHtml('folder')).toContain('src="icons/folder.png"');
    expect(iconHtml('file')).toContain('src="icons/file-dark.png"');
  });

  it('passes through an unmapped but well-formed name', () => {
    expect(iconHtml('custom-icon')).toContain('src="icons/custom-icon.png"');
  });

  it('rejects a name that breaks out of the src attribute', () => {
    expect(iconHtml('x.png" onerror="alert(1)')).toBe('');
  });

  it('rejects a name that closes the tag', () => {
    expect(iconHtml('x"><script>alert(1)</script>')).toBe('');
  });

  it('rejects path separators', () => {
    expect(iconHtml('../../etc/passwd')).toBe('');
    expect(iconHtml('sub/dir')).toBe('');
  });

  it('rejects an absolute URL', () => {
    expect(iconHtml('https://evil.example/x')).toBe('');
  });

  it('returns empty for a missing name', () => {
    expect(iconHtml('')).toBe('');
    expect(iconHtml(null)).toBe('');
    expect(iconHtml(undefined)).toBe('');
  });
});

describe('iconHtml — size handling', () => {
  it('defaults to 14px', () => {
    expect(iconHtml('folder')).toContain('width="14"');
  });

  it('uses a numeric size', () => {
    expect(iconHtml('folder', 20)).toContain('width="20"');
  });

  it('falls back to the default for a non-numeric size', () => {
    const out = iconHtml('folder', '16" onload="alert(1)');
    expect(out).toContain('width="14"');
    expect(out).not.toContain('onload');
  });
});

import { describe, it, expect } from 'vitest';

import './html-sanitizer.js';

/** Sanitize and return the resulting markup as a string, for assertions. */
function clean(html) {
  const host = document.createElement('div');
  host.appendChild(window.htmlSanitizer.sanitizeToFragment(html, document));
  return host.innerHTML;
}

/** Sanitize and return the container element, for structural assertions. */
function cleanEl(html) {
  const host = document.createElement('div');
  host.appendChild(window.htmlSanitizer.sanitizeToFragment(html, document));
  return host;
}

describe('sanitizeToFragment — script execution vectors', () => {
  it('strips inline event handler attributes', () => {
    const out = clean('<img src="x.png" onerror="alert(1)">');
    expect(out).not.toContain('onerror');
    expect(out).toContain('<img');
  });

  it('strips event handlers regardless of attribute case', () => {
    const out = clean('<div ONCLICK="alert(1)" OnMouseOver="alert(2)">hi</div>');
    expect(out.toLowerCase()).not.toContain('onclick');
    expect(out.toLowerCase()).not.toContain('onmouseover');
    expect(out).toContain('hi');
  });

  it('drops script elements and their contents', () => {
    const out = clean('<div>before</div><script>alert(1)</script><div>after</div>');
    expect(out).not.toContain('alert');
    expect(out).not.toContain('script');
    expect(out).toContain('before');
    expect(out).toContain('after');
  });

  it('drops event handlers nested deep in the tree', () => {
    const out = clean('<div><ul><li><span onclick="alert(1)">x</span></li></ul></div>');
    expect(out).not.toContain('onclick');
    expect(out).toContain('x');
  });

  it('leaves no on* attribute anywhere in a mixed payload', () => {
    const host = cleanEl(
      '<div onclick="a()"><img src="x.png" onerror="b()"><a href="#" onfocus="c()">l</a></div>'
    );
    for (const el of host.querySelectorAll('*')) {
      for (const attribute of el.attributes) {
        expect(attribute.name.startsWith('on')).toBe(false);
      }
    }
  });
});

describe('sanitizeToFragment — URL schemes', () => {
  it('removes javascript: hrefs', () => {
    const out = clean('<a href="javascript:alert(1)">click</a>');
    expect(out).not.toContain('javascript:');
    expect(out).toContain('click');
  });

  // Tab, LF, and CR are stripped by the URL parser before the scheme is read,
  // so they are the characters an attacker splits `javascript:` with.
  it.each([
    ['tab', '\t'],
    ['newline', '\n'],
    ['carriage return', '\r'],
  ])('removes javascript: split by a %s', (_name, char) => {
    const out = clean(`<a href="java${char}script:alert(1)">click</a>`);
    expect(out.toLowerCase()).not.toContain('script:');
    expect(out).toContain('click');
  });

  it('removes javascript: padded with leading whitespace', () => {
    const out = clean('<a href="   javascript:alert(1)">click</a>');
    expect(out.toLowerCase()).not.toContain('javascript:');
  });

  it('removes vbscript: hrefs', () => {
    const out = clean('<a href="vbscript:msgbox(1)">click</a>');
    expect(out).not.toContain('vbscript');
  });

  it('keeps http, https, and mailto hrefs', () => {
    expect(clean('<a href="https://example.com">x</a>')).toContain('https://example.com');
    expect(clean('<a href="http://example.com">x</a>')).toContain('http://example.com');
    expect(clean('<a href="mailto:a@b.c">x</a>')).toContain('mailto:a@b.c');
  });

  it('keeps relative URLs', () => {
    expect(clean('<img src="icons/file.png">')).toContain('icons/file.png');
    expect(clean('<a href="#section">x</a>')).toContain('#section');
  });

  it('treats a colon after a path segment as relative, not a scheme', () => {
    expect(clean('<a href="foo/bar:baz">x</a>')).toContain('foo/bar:baz');
  });

  it('allows raster data URIs on img but not svg', () => {
    const png = 'data:image/png;base64,iVBORw0KGgo=';
    expect(clean(`<img src="${png}">`)).toContain(png);

    const svg = 'data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=';
    expect(clean(`<img src="${svg}">`)).not.toContain('svg+xml');
  });

  it('rejects data URIs on anchors', () => {
    const out = clean('<a href="data:text/html;base64,PHNjcmlwdD4=">x</a>');
    expect(out).not.toContain('data:');
  });
});

describe('sanitizeToFragment — element allowlist', () => {
  it.each(['iframe', 'object', 'embed', 'form', 'input', 'style', 'template'])(
    'drops <%s>',
    (tag) => {
      const out = clean(`<${tag}></${tag}>`);
      expect(out).not.toContain(`<${tag}`);
    }
  );

  it('drops math foreign content', () => {
    expect(clean('<math><mi>x</mi></math>')).not.toContain('math');
  });

  it('keeps buttons, which plugin panels are built from', () => {
    const host = cleanEl('<button class="b" data-action="refresh" aria-label="Refresh">go</button>');
    const el = host.querySelector('button');
    expect(el).not.toBeNull();
    expect(el.getAttribute('data-action')).toBe('refresh');
    expect(el.getAttribute('aria-label')).toBe('Refresh');
  });
});

describe('sanitizeToFragment — inline SVG', () => {
  it('keeps an icon and preserves case-sensitive attribute names', () => {
    const host = cleanEl(
      '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5h2v6z"></path></svg>'
    );
    const svg = host.querySelector('svg');
    expect(svg).not.toBeNull();
    expect(svg.getAttribute('viewBox')).toBe('0 0 24 24');
    expect(svg.namespaceURI).toBe('http://www.w3.org/2000/svg');
    expect(host.querySelector('path').getAttribute('d')).toBe('M11 5h2v6z');
  });

  it.each(['script', 'foreignObject', 'use', 'animate', 'set', 'image'])(
    'drops <%s> inside svg',
    (tag) => {
      const out = clean(`<svg><${tag}></${tag}></svg>`);
      expect(out.toLowerCase()).not.toContain(`<${tag.toLowerCase()}`);
      expect(out).toContain('<svg');
    }
  );

  it('drops event handlers on svg elements', () => {
    const out = clean('<svg onload="alert(1)"><path d="M0 0" onclick="alert(2)"></path></svg>');
    expect(out).not.toContain('onload');
    expect(out).not.toContain('onclick');
  });

  it('drops URL-bearing attributes on svg elements', () => {
    const out = clean('<svg><path d="M0 0" href="javascript:alert(1)"></path></svg>');
    expect(out).not.toContain('javascript:');
    expect(out).not.toContain('href');
  });

  it('drops comments', () => {
    expect(clean('<div>a</div><!-- secret -->')).not.toContain('secret');
  });

  it('keeps ordinary structural markup', () => {
    const out = clean('<div class="row"><span>hello</span></div>');
    expect(out).toContain('class="row"');
    expect(out).toContain('<span>hello</span>');
  });

  it('keeps tables', () => {
    const host = cleanEl('<table><tbody><tr><td colspan="2">c</td></tr></tbody></table>');
    expect(host.querySelector('td').getAttribute('colspan')).toBe('2');
  });
});

describe('sanitizeToFragment — attributes', () => {
  it('preserves data-action attributes that drive plugin click wiring', () => {
    const host = cleanEl('<div data-action="refresh" data-dbl-action="open">x</div>');
    const el = host.querySelector('[data-action]');
    expect(el.getAttribute('data-action')).toBe('refresh');
    expect(el.getAttribute('data-dbl-action')).toBe('open');
  });

  it('drops attributes outside the allowlist', () => {
    const out = clean('<img src="x.png" srcset="evil.png 2x" formaction="y">');
    expect(out).not.toContain('srcset');
    expect(out).not.toContain('formaction');
  });

  it('adds rel=noopener when a link targets another context', () => {
    const host = cleanEl('<a href="https://example.com" target="_blank">x</a>');
    expect(host.querySelector('a').getAttribute('rel')).toBe('noopener noreferrer');
  });

  it('keeps class, id, title, and style', () => {
    const out = clean('<div class="a" id="b" title="c" style="color:red">x</div>');
    expect(out).toContain('class="a"');
    expect(out).toContain('id="b"');
    expect(out).toContain('title="c"');
    expect(out).toContain('style="color:red"');
  });
});

describe('sanitizeToFragment — input handling', () => {
  it('returns an empty fragment for null, undefined, and empty input', () => {
    expect(clean(null)).toBe('');
    expect(clean(undefined)).toBe('');
    expect(clean('')).toBe('');
  });

  it('preserves plain text', () => {
    expect(clean('just text')).toBe('just text');
  });

  it('escapes text that looks like markup rather than reviving it', () => {
    const out = clean('<div>&lt;script&gt;alert(1)&lt;/script&gt;</div>');
    expect(out).not.toContain('<script>');
    expect(out).toContain('&lt;script&gt;');
  });
});

describe('sanitizeToFragment — real plugin markup', () => {
  // Lifted from examples/plugins/lua-tmux-manager.lua, the one in-repo plugin
  // that builds its whole UI from the html widget. Sanitizing must not gut it.
  const TOOLBAR = `
    <div class="tmx-toolbar">
      <button class="tmx-icon-btn" data-action="refresh" title="Refresh sessions" aria-label="Refresh sessions">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5a7 7 0 0 1 6.93 6h-2.18z"></path></svg>
      </button>
      <button class="tmx-row-main" data-action="select_session:0" data-context-action="show_session_menu:0" title="Single click selects.">
        <span class="tmx-row-label">main</span>
      </button>
    </div>`;

  it('preserves structure, actions, labels, and icons', () => {
    const host = cleanEl(TOOLBAR);
    expect(host.querySelectorAll('button')).toHaveLength(2);
    expect(host.querySelector('[data-action="refresh"]')).not.toBeNull();
    expect(host.querySelector('[data-context-action="show_session_menu:0"]')).not.toBeNull();
    expect(host.querySelector('svg path').getAttribute('d')).toContain('M12 5a7');
    expect(host.querySelector('.tmx-row-label').textContent).toBe('main');
    expect(host.querySelector('.tmx-toolbar')).not.toBeNull();
  });
});

describe('isSafeUrl', () => {
  it('accepts relative and inert absolute URLs', () => {
    expect(window.htmlSanitizer.isSafeUrl('icons/a.png')).toBe(true);
    expect(window.htmlSanitizer.isSafeUrl('https://example.com')).toBe(true);
  });

  it('rejects script schemes and empty values', () => {
    expect(window.htmlSanitizer.isSafeUrl('javascript:alert(1)')).toBe(false);
    expect(window.htmlSanitizer.isSafeUrl('')).toBe(false);
    expect(window.htmlSanitizer.isSafeUrl(null)).toBe(false);
  });
});

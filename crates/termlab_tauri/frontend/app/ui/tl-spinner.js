// Numeric spinner control (window.tlSpinner) — wraps an EXISTING native
// <input type="number"> the same way app/ui/tl-combo.js wraps a <select>:
// the input stays in the DOM as the source of truth (its own `.value`,
// `min`/`max`/`step` attributes, and any `input`/`change` handlers keep
// working), attach(inputEl) just adds a custom-styled stepper column next
// to it.
//
// This exists because the native browser spin buttons can't be restyled to
// match the design system (no way to swap in a vendored icon), but per the
// task-2 review's guidance the actual value-changing logic stays on the
// native input: the stepper buttons call inputEl.stepUp()/stepDown() (the
// browser's own min/max/step-aware arithmetic) rather than reimplementing
// it, falling back to plain arithmetic only if those aren't available.
//
// Nothing consumes this module yet (design-system-phase-5a task 4 migrates
// the Add SSH Tunnel dialog's port fields onto it).
(function initTermLabSpinner(global) {
  'use strict';

  function makeStepButton(direction, ariaLabel) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'tl-spinner__step tl-spinner__step--' + direction;
    // Keep normal Tab order landing on the input, not these buttons — same
    // as a native spinner's up/down arrows, which aren't separately
    // tab-stoppable either.
    btn.tabIndex = -1;
    btn.setAttribute('aria-label', ariaLabel);
    if (global.tlIcon && typeof global.tlIcon.create === 'function') {
      btn.appendChild(global.tlIcon.create('chevronDown', { size: 10, alt: '' }));
    }
    return btn;
  }

  function attach(inputEl) {
    if (!inputEl || inputEl.tagName !== 'INPUT') return null;
    // Idempotent: a second attach() on the same input returns the existing
    // api instead of wrapping it twice.
    if (inputEl._tlSpinner) return inputEl._tlSpinner;

    if (!inputEl.type || inputEl.type === 'text') inputEl.type = 'number';
    inputEl.classList.add('tl-spinner__input');

    const wrap = document.createElement('div');
    wrap.className = 'tl-spinner';
    // Move the input inside the wrapper without changing its position in
    // the surrounding markup: insert the (empty) wrapper right before the
    // input, then move the input into it.
    inputEl.insertAdjacentElement('beforebegin', wrap);
    wrap.appendChild(inputEl);

    const stepper = document.createElement('div');
    stepper.className = 'tl-spinner__stepper';
    const upBtn = makeStepButton('up', 'Increase');
    const downBtn = makeStepButton('down', 'Decrease');
    stepper.appendChild(upBtn);
    stepper.appendChild(downBtn);
    wrap.appendChild(stepper);

    function refresh() {
      upBtn.disabled = !!inputEl.disabled;
      downBtn.disabled = !!inputEl.disabled;
    }
    refresh();

    function step(direction) {
      if (inputEl.disabled) return;
      try {
        if (direction > 0 && typeof inputEl.stepUp === 'function') {
          inputEl.stepUp();
        } else if (direction < 0 && typeof inputEl.stepDown === 'function') {
          inputEl.stepDown();
        } else {
          throw new Error('native stepUp/stepDown unavailable');
        }
      } catch (_) {
        // stepUp/stepDown throw on values that can't be stepped (e.g. an
        // empty or non-numeric current value in some engines) — fall back
        // to plain arithmetic so the button still does something sensible.
        const amount = parseFloat(inputEl.step) || 1;
        const current = parseFloat(inputEl.value) || 0;
        inputEl.value = String(current + direction * amount);
      }
      // Dispatch both events so existing/future handlers see the change
      // exactly as they would from typing or using the native stepper.
      inputEl.dispatchEvent(new Event('input', { bubbles: true }));
      inputEl.dispatchEvent(new Event('change', { bubbles: true }));
    }

    upBtn.addEventListener('click', () => step(1));
    downBtn.addEventListener('click', () => step(-1));

    // Keep the stepper buttons' disabled state in sync with the input for
    // changes that happen after attach() (e.g. code that toggles
    // `input.disabled` directly), same pattern as tl-combo.js.
    if (global.MutationObserver) {
      const observer = new global.MutationObserver(refresh);
      observer.observe(inputEl, { attributes: true, attributeFilter: ['disabled'] });
    }

    const api = { wrap, upBtn, downBtn, refresh };
    inputEl._tlSpinner = api;
    return api;
  }

  global.tlSpinner = { attach };
})(window);

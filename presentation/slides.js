const slides = [...document.querySelectorAll('.slide')];
const counter = document.querySelector('#counter');
const progress = document.querySelector('#progress-line');
const previous = document.querySelector('#prev');
const next = document.querySelector('#next');
let current = 0;

function setCurrent(index, updateHash) {
  current = index;
  const slide = slides[index];
  counter.value = `${String(current + 1).padStart(2, '0')} / ${String(slides.length).padStart(2, '0')}`;
  progress.style.width = `${((current + 1) / slides.length) * 100}%`;
  previous.disabled = current === 0;
  next.disabled = current === slides.length - 1;
  if (updateHash) history.replaceState(null, '', `#${slide.id}`);
}

function show(index, updateHash = true) {
  const target = Math.max(0, Math.min(index, slides.length - 1));
  setCurrent(target, updateHash);
  slides[target].scrollIntoView({ behavior: 'instant', block: 'start' });
}

previous.addEventListener('click', () => show(current - 1));
next.addEventListener('click', () => show(current + 1));

document.addEventListener('keydown', (event) => {
  if (
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    /^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName)
  )
    return;
  if (['ArrowRight', 'ArrowDown', 'PageDown', ' '].includes(event.key)) {
    event.preventDefault();
    show(current + 1);
  } else if (['ArrowLeft', 'ArrowUp', 'PageUp'].includes(event.key)) {
    event.preventDefault();
    show(current - 1);
  } else if (event.key === 'Home') {
    event.preventDefault();
    show(0);
  } else if (event.key === 'End') {
    event.preventDefault();
    show(slides.length - 1);
  }
});

const initial = slides.findIndex((slide) => `#${slide.id}` === location.hash);
show(initial < 0 ? 0 : initial, false);
window.addEventListener('hashchange', () => {
  const target = slides.findIndex((slide) => `#${slide.id}` === location.hash);
  if (target >= 0) show(target, false);
});

let scrollFrame = 0;
window.addEventListener('scroll', () => {
  if (scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = 0;
    const position = window.scrollY;
    let closest = 0;
    let distance = Infinity;
    slides.forEach((slide, index) => {
      const offset = Math.abs(slide.offsetTop - position);
      if (offset < distance) {
        distance = offset;
        closest = index;
      }
    });
    if (closest !== current) setCurrent(closest, true);
  });
});

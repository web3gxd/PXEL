const slides = [...document.querySelectorAll('.slide')];
const counter = document.querySelector('#counter');
const progress = document.querySelector('#progress-line');
const previous = document.querySelector('#prev');
const next = document.querySelector('#next');
let current = 0;

function show(index, updateHash = true) {
  current = Math.max(0, Math.min(index, slides.length - 1));
  const slide = slides[current];
  counter.value = `${String(current + 1).padStart(2, '0')} / ${String(slides.length).padStart(2, '0')}`;
  progress.style.width = `${((current + 1) / slides.length) * 100}%`;
  previous.disabled = current === 0;
  next.disabled = current === slides.length - 1;
  slide.scrollIntoView({ behavior: updateHash ? 'smooth' : 'instant', block: 'start' });
  if (updateHash) history.replaceState(null, '', `#${slide.id}`);
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

const observer = new IntersectionObserver(
  (entries) => {
    const visible = entries
      .filter((entry) => entry.isIntersecting)
      .sort((a, b) => b.intersectionRatio - a.intersectionRatio);
    if (!visible.length) return;
    const index = slides.indexOf(visible[0].target);
    if (index !== current) {
      current = index;
      counter.value = `${String(index + 1).padStart(2, '0')} / ${String(slides.length).padStart(2, '0')}`;
      progress.style.width = `${((index + 1) / slides.length) * 100}%`;
      previous.disabled = index === 0;
      next.disabled = index === slides.length - 1;
      history.replaceState(null, '', `#${slides[index].id}`);
    }
  },
  { threshold: [0.45, 0.75] },
);
slides.forEach((slide) => observer.observe(slide));

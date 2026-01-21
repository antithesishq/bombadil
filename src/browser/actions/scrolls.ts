import { Actions } from "../actions";

result = (() => {
  const scrolls: Actions = [];

  if (window.scrollY > 0) {
    scrolls.push([1, 100, {
      ScrollUp: {
        origin: {
          x: window.innerWidth / 2,
          y: window.innerHeight / 2,
        },
        distance: Math.min(window.innerHeight / 2, window.scrollY),
      }
    }]);
  }

  const scroll_y_max = document.body.scrollHeight - window.innerHeight;
  const scroll_y_max_diff = scroll_y_max - window.scrollY;
  if (scroll_y_max_diff >= 1) {
    scrolls.push([10, 100, {
      ScrollDown: {
        origin: {
          x: window.innerWidth / 2,
          y: window.innerHeight / 2,
        },
        distance: Math.min(window.innerHeight / 2, scroll_y_max_diff),
      }
    }]);
  }
  return scrolls;
})();

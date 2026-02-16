import { Actions } from "../actions";

result = (() => {
  const scrolls: Actions = [];

  if (document.body) {
    const scroll_y_max = document.body.scrollHeight - window.innerHeight;
    const scroll_y_max_diff = scroll_y_max - window.scrollY;

    if (scroll_y_max_diff >= 1) {
      // Not at the bottom yet — scroll down
      scrolls.push([
        10,
        100,
        {
          ScrollDown: {
            origin: {
              x: window.innerWidth / 2,
              y: window.innerHeight / 2,
            },
            distance: Math.min(window.innerHeight / 2, scroll_y_max_diff),
          },
        },
      ]);
    } else if (window.scrollY > 0) {
      // At the bottom — scroll to top
      scrolls.push([
        1,
        100,
        {
          ScrollUp: {
            origin: {
              x: window.innerWidth / 2,
              y: window.innerHeight / 2,
            },
            distance: window.scrollY,
          },
        },
      ]);
    }
  }

  return scrolls;
})();

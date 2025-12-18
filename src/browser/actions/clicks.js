function clicks() {
  function clickable_point(element) {
    // naive calculation of center of element
    const rect = element.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      return { x: rect.left + (rect.width / 2), y: rect.top + (rect.height / 2) };
    } else {
      return null;
    }
  }

  function is_visible(element) {
    const style = window.getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" && parseFloat(style.opacity || "1")  > 0.0;
  }

  function contains(rect, point) {
    return point.x >= rect.x && 
      point.x <= (rect.x + rect.width) && 
      point.y >= rect.y && 
      point.y <= (rect.y + rect.height);
  }

  const clicks = [];
  const url_current = new URL(window.location);
  for (const anchor of document.querySelectorAll("a")) {
    try {
      let url;
      try {
        url = new URL(anchor.href);
      } catch (e) {
        console.debug(anchor.href, "could not be parsed as a URL:", e);
        continue;
      }

      if (anchor.target === "_blank") {
        continue;
      }

      if (!url.protocol.startsWith('http')) {
        console.debug(url, "is not an http(s) URL");
        continue;
      }
  
      if (!url.origin.endsWith(url_current.origin)) {
        console.debug(url, "is not within domain", url_current.origin);
        continue;
      }

      if (!is_visible(anchor)) {
        console.debug(anchor, "is not visible");
        continue;
      }
          
      const point = clickable_point(anchor);
      if (!point) {
        console.debug(anchor, "is not clickable");
        continue;
      }

      const viewport = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
      if (!contains(viewport, point)) {
        console.debug(anchor, "is out of viewport");
        continue;
      }

      clicks.push([1, 500, {
        Click: {
          name: anchor.nodeName,
          content: anchor.textContent.trim().replaceAll(/\s+/g, " "),
          point,
        }
      }]);
    } catch (e) {
      console.error(e);
      continue;
    }
  }
  for (const element of document.querySelectorAll("button,input,textarea,label[for]")) {
    try {
      // We require visibility except for input elements, which are often hidden and overlayed with custom styling.
      if (!(element instanceof HTMLInputElement) && !is_visible(element)) {
        console.debug(element, "is not visible");
        continue;
      }
          
      const point = clickable_point(element);
      if (!point) {
        console.debug(element, "is not clickable");
        continue;
      }

      const viewport = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
      if (!contains(viewport, point)) {
        console.debug(element, "is out of viewport");
        continue;
      }

      clicks.push([3, 300, {
        Click: {
          name: element.nodeName,
          content: element.textContent.trim().replaceAll(/\s+/g, " "),
          point,
        }
      }]);
    } catch (e) {
      console.error(e);
      continue;
    }
  }
  return clicks;
}

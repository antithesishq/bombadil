document.addEventListener('DOMContentLoaded', function() {
  const toc = document.getElementById('TOC');
  if (!toc) return;

  const toggleBtn = toc.querySelector('header .btn');
  if (!toggleBtn) return;

  toggleBtn.addEventListener('click', function(e) {
    e.preventDefault();
    toc.classList.toggle('expanded');
  });

  document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape' && toc.classList.contains('expanded')) {
      toc.classList.remove('expanded');
    }
  });

  toc.addEventListener('click', function(e) {
    if (e.target.tagName === 'A' && e.target.hash) {
      toc.classList.remove('expanded');
    }
  });
});

#!/usr/bin/env python3
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlparse, parse_qs

# usage: python3 trace_to_d3_elk.py trace.jsonl /path/to/output_dir
trace_file = sys.argv[1]
out_dir = Path(sys.argv[2])
img_dir = out_dir / "images"
out_dir.mkdir(parents=True, exist_ok=True)
img_dir.mkdir(parents=True, exist_ok=True)

# ---------- load trace ----------
trace = []
with open(trace_file) as f:
    for line in f:
        t = json.loads(line)
        trace.append(
            {
                "url": t.get("url"),
                "prev": t.get("hash_previous"),
                "curr": t.get("hash_current"),
                "screenshot": t.get("screenshot"),
                "action": t.get("action"),
            }
        )


def split_url(url):
    """Return path segments, query parameters dict, and fragment as a feature"""
    parsed = urlparse(url)
    path_segments = parsed.path.strip("/").split("/")
    query_params = parse_qs(parsed.query)
    fragment = parsed.fragment
    return path_segments, query_params, fragment


UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-"
    r"[0-9a-fA-F]{4}-"
    r"[0-9a-fA-F]{4}-"
    r"[0-9a-fA-F]{4}-"
    r"[0-9a-fA-F]{12}$"
)


def segment_weight(segment=None, param_name=None):
    """Assign weight: path=1, numeric/UUID=0.5, query params important=0.5, else 0"""
    if param_name:
        return 0.5 if param_name in ["page", "lang"] else 0
    if segment is None:
        return 1
    if segment.isdigit() or UUID_RE.match(segment):
        return 0.5
    return 1


def feature_hash(s):
    """Simple 64-bit hash"""
    h = 0xCBF29CE484222325
    fnv_prime = 0x100000001B3
    for c in s:
        h ^= ord(c)
        h *= fnv_prime
        h &= 0xFFFFFFFFFFFFFFFF
    return h


def url_features(url):
    parsed = urlparse(url)
    path_segments = parsed.path.strip("/").split("/")
    query_params = parse_qs(parsed.query)
    fragment = parsed.fragment
    features = []
    # path
    segment_count = len(path_segments)
    for i, seg in enumerate(path_segments):
        if seg:
            w = int(segment_weight(seg) * (((i + 1) / segment_count + 0.5) ** 2)) or 1
            features.extend([seg] * w)
    # query
    for k, vals in query_params.items():
        w = segment_weight(None, k)
        if w > 0:
            for v in vals:
                features.extend([f"{k}={v}"] * int(w * 10))
    # fragment
    if fragment:
        features.extend([fragment] * 5)  # moderate weight
    return features


def url_simhash(url):
    feats = url_features(url)
    v = [0] * 64
    for f in feats:
        h = feature_hash(f)
        for i in range(64):
            v[i] += 1 if (h >> i) & 1 else -1
    sim = 0
    for i, val in enumerate(v):
        if val > 0:
            sim |= 1 << i
    return sim


# ---------- compute simhashes for trace ----------
for t in trace:
    # coverage hash must exist for original clustering
    t["cov_hash"] = t.get("curr") or 0  # or whatever coverage integer
    url = t.get("url")
    if url:
        t["url_simhash"] = url_simhash(url)
    else:
        t["url_simhash"] = 0

# ---------- weighted clustering ----------
coverage_weight = 1.0
url_weight = 2.0
THRESHOLD = 8  # adjust as needed


def combined_distance(a, b):
    """Weighted sum of coverage Hamming and URL simhash Hamming"""
    d_cov = bin(a["cov_hash"] ^ b["cov_hash"]).count("1")
    d_url = bin(a.get("url_simhash", 0) ^ b.get("url_simhash", 0)).count("1")
    total_weight = coverage_weight + url_weight
    return (coverage_weight * d_cov + url_weight * d_url) / total_weight


clusters = []
node_to_cluster = {}

for idx, t in enumerate(trace):
    for ci, cluster in enumerate(clusters):
        rep_idx = cluster[0]
        rep = trace[rep_idx]
        if combined_distance(t, rep) <= THRESHOLD:
            cluster.append(idx)
            node_to_cluster[idx] = ci
            break
    else:
        # new cluster
        clusters.append([idx])
        node_to_cluster[idx] = len(clusters) - 1


first_cluster = node_to_cluster[0]

cluster_images = {}
cluster_all_images = {}

for ci, cluster in enumerate(clusters):
    # representative image = first non-empty screenshot
    cluster_images[ci] = next(
        (
            trace[idx].get("screenshot")
            for idx in cluster
            if trace[idx].get("screenshot")
        ),
        None,
    )
    # all images in cluster
    cluster_all_images[ci] = [
        trace[idx].get("screenshot") for idx in cluster if trace[idx].get("screenshot")
    ]


# ---------- summarize actions ----------
def summarize_action(action):
    if not action:
        return "?"
    variant = next(iter(action.keys()))
    data = action[variant]
    if variant == "Click":
        name = data.get("name", "?")
        content = data.get("content")
        return f"Click({content})" if content else f"Click({name})"
    if variant == "TypeText":
        return f"Type"
    if variant == "PressKey":
        return f"Key({data.get('code')})"
    return variant


# ---------- edges ----------
edges = []
seen = set()
last_idx = None  # last trace index

for idx, t in enumerate(trace):
    curr_idx = idx
    prev_idx = last_idx

    if prev_idx is None:
        last_idx = curr_idx
        continue

    ci = node_to_cluster[prev_idx]
    cj = node_to_cluster[curr_idx]
    label = summarize_action(t.get("action"))

    # key to deduplicate edges between clusters with the same label
    key = (ci, cj, label)
    if key not in seen:
        edges.append(
            {
                "id": f"e{ci}_{cj}_{len(edges)}",
                "sources": [str(ci)],
                "targets": [str(cj)],
                "label": label,
            }
        )
        seen.add(key)

    last_idx = curr_idx

# ---------- ELK graph ----------
elk_graph = {
    "id": "root",
    "layoutOptions": {
        "elk.algorithm": "layered",
        "elk.direction": "RIGHT",
        "elk.layered.spacing.nodeNodeBetweenLayers": "200",
        "elk.spacing.nodeNode": "160",
        "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
        "elk.edgeRouting": "ORTHOGONAL",
        "elk.layered.nodePlacement.strategy": "INTERACTIVE",
    },
    "children": [
        {
            "id": str(ci),
            "width": 160,
            "height": 120,
            "label": f"{'START ' if ci==first_cluster else ''}({len(cluster_all_images[ci])})",
            "image": cluster_images.get(ci),
            "screenshots": cluster_all_images.get(ci, []),
            "layoutOptions": (
                {
                    "elk.layered.layerConstraint": "FIRST",  # force to first layer
                }
                if ci == first_cluster
                else {}
            ),
        }
        for ci, _ in enumerate(clusters)
    ],
    "edges": edges,
}

# ---------- HTML ----------
html = f"""<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8"/>
<title>State Graph (ELK)</title>

<script src="https://unpkg.com/elkjs@0.9.3/lib/elk.bundled.js"></script>
<script src="https://d3js.org/d3.v7.min.js"></script>

<style>
html, body {{
  margin: 0;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: #111;
  font-family: sans-serif;
}}

svg {{
  width: 100vw;
  height: 100vh;
}}

.node rect {{
  fill: #222;
  stroke: #666;
  stroke-width: 2px;
  rx: 8;
}}

.node text {{
  fill: white;
  font-size: 12px;
  text-anchor: middle;
}}

.node {{
  cursor: pointer;
}}

.edge-path {{
  stroke: #aaa;
  stroke-width: 2px;
  fill: none;
}}

.edge-label {{
  fill: #ddd;
  font-size: 11px;
  pointer-events: none;
}}

#overlay {{
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.9);
  display: none;
  z-index: 10;
}}

#overlay-close {{
  position: absolute;
  top: 10px;
  right: 20px;
  color: white;
  font-size: 24px;
  cursor: pointer;
}}

#overlay-content {{
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 16px;
  padding: 60px 20px 20px;
  overflow-y: auto;
  height: 100%;
}}
</style>
</head>
<body>

<svg><g></g></svg>

<div id="overlay">
  <div id="overlay-close">✕</div>
  <div id="overlay-content"></div>
</div>

<script>
const elkGraph = {json.dumps(elk_graph)};
const elk = new ELK();
const svg = d3.select("svg");
const g = svg.select("g");

// arrowhead
svg.append("defs")
  .append("marker")
  .attr("id", "arrowhead")
  .attr("viewBox", "0 -5 10 10")
  .attr("refX", 10)
  .attr("refY", 0)
  .attr("markerWidth", 6)
  .attr("markerHeight", 6)
  .attr("orient", "auto")
  .append("path")
  .attr("d", "M0,-5L10,0L0,5")
  .attr("fill", "#aaa");

svg.call(
  d3.zoom().scaleExtent([0.1, 4]).on("zoom", e => {{
    g.attr("transform", e.transform);
  }})
);

const overlay = document.getElementById("overlay");
const overlayContent = document.getElementById("overlay-content");
document.getElementById("overlay-close").onclick = () => {{
  overlay.style.display = "none";
  overlayContent.innerHTML = "";
}};
overlay.onclick = e => {{
  if (e.target === overlay) {{
    overlay.style.display = "none";
    overlayContent.innerHTML = "";
  }}
}};
document.addEventListener("keydown", (e) => {{
  if (e.key === "Escape") {{
    overlay.style.display = "none";
    overlayContent.innerHTML = "";
  }}
}});

function showScreenshots(node) {{
  overlayContent.innerHTML = "";
  (node.screenshots || []).forEach(src => {{
    const img = document.createElement("img");
    img.src = src;
    img.style.width = "100%";
    img.style.border = "1px solid #444";
    overlayContent.appendChild(img);
  }});
  overlay.style.display = "block";
}}

elk.layout(elkGraph).then(layout => {{

  const edgeGroup = g.append("g");
  layout.edges.forEach(e => {{
    e.sections.forEach(s => {{
      const pts = [s.startPoint, ...(s.bendPoints || []), s.endPoint];
  
      // Edge path
      edgeGroup.selectAll(".edge-path")
        .data(layout.edges.flatMap(e => e.sections.map(s => ({{ edge: e, section: s }}))))
        .enter()
        .append("path")
        .attr("class", "edge-path")
        .attr("data-source", d => d.edge.sources[0].toString())
        .attr("data-target", d => d.edge.targets[0].toString())
        .attr("marker-end", "url(#arrowhead)")
        .attr("d", d => {{
          const points = [d.section.startPoint, ...(d.section.bendPoints || []), d.section.endPoint];
          return d3.line().x(p => p.x).y(p => p.y)(points);
        }});
  
      // Edge label
      if (e.label) {{
        const start = pts[0];
        const end = pts[pts.length - 1];

        let x, y, anchor, baseline;
        if (e.sources[0] === e.targets[0]) {{
            // Self-loop: place label above the loop
            x = start.x;                      // center horizontally
            y = start.y - 20;                 // 20px above the node (adjust as needed)
            anchor = "middle";                // center the text
            baseline = "auto";
        }} else {{
            // Normal edge: align based on direction
            anchor = end.x < start.x ? "start" : "end";
            const offset = anchor === "start" ? 6 : -6;
            x = end.x + offset;
            y = end.y;
            baseline = "middle";
        }}

        edgeGroup.append("text")
          .attr("class","edge-label")
          .attr("data-source", e.sources[0].toString())
          .attr("data-target", e.targets[0].toString())
          .attr("x", x)
          .attr("y", y)
          .attr("text-anchor", anchor)
          .attr("dominant-baseline", baseline)
          .text(e.label);

      }}
     }});
  }});

  const nodes = g.append("g")
    .selectAll(".node")
    .data(layout.children)
    .enter()
    .append("g")
    .attr("class", "node")
    .attr("transform", d => `translate(${{d.x}},${{d.y}})`)
    .on("click", (e,d) => {{
      e.stopPropagation();
      showScreenshots(d);
    }});

  nodes.append("rect")
    .attr("width", d => d.width)
    .attr("height", d => d.height);

  nodes.append("image")
    .attr("href", d => d.image || "")
    .attr("x", 10)
    .attr("y", 10)
    .attr("width", 140)
    .attr("height", 80);

  nodes.append("text")
    .attr("x", d => d.width/2)
    .attr("y", d => d.height - 10)
    .text(d => d.label);

  const padding = 2;
  edgeGroup.selectAll("text").each(function() {{
    const text = d3.select(this);        // current text element
    const bbox = text.node().getBBox();  // bounding box of this text
  
    // Insert rect behind the text
    d3.select(this.parentNode)            // insert inside same group
      .insert("rect", function() {{ return text.node(); }}) // insert before this text
      .attr("class", "edge-bg")
      .attr("x", bbox.x - padding)
      .attr("y", bbox.y - padding)
      .attr("width", bbox.width + padding*2)
      .attr("height", bbox.height + padding*2)
      .style("fill", "black")
      .style("opacity", "0.5");
  }});

  nodes
    .on("mouseover", function(event, d) {{
      const cid = d.id;
  
      // highlight only edges connected to this node
      edgeGroup.selectAll(".edge-path")
        .style("opacity", e => {{
          return (e.edge.sources[0] === cid || e.edge.targets[0] === cid) ? 1 : 0.2;
        }});

      // labels
      edgeGroup.selectAll(".edge-label")
        .style("opacity", function(l) {{
          // get the dataset attributes from the DOM element
          const source = this.dataset.source;
          const target = this.dataset.target;
          return (source === cid || target === cid) ? 1 : 0.2;
      }});
  
      // optionally dim other nodes too
      nodes.style("opacity", n => (n.id === cid ? 1 : 0.5));
    }})
    .on("mouseout", function() {{
      // reset opacity
      edgeGroup.selectAll(".edge-path")
        .style("opacity", 1)
        .style("stroke-width", 2);

      edgeGroup.selectAll(".edge-label")
        .style("opacity", 1);
  
      nodes.style("opacity", 1);
    }});

  const bbox = g.node().getBBox();
  const scale = Math.min(innerWidth/bbox.width, innerHeight/bbox.height)*0.9;
  svg.call(
    d3.zoom().transform,
    d3.zoomIdentity
      .translate((innerWidth-bbox.width*scale)/2-bbox.x*scale,
                 (innerHeight-bbox.height*scale)/2-bbox.y*scale)
      .scale(scale)
  );
}});
</script>
</body>
</html>
"""

(out_dir / "index.html").write_text(html, encoding="utf-8")
print(f"Output written to {out_dir}")
print("Serve with: python3 -m http.server")

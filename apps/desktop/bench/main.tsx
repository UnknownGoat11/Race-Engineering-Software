// Entry of the graph renderer benchmark page (see Bench.tsx). SYNTHETIC BENCHMARK DATA only.
import { createRoot } from 'react-dom/client';
import '../src/styles/tokens.css';
import '../src/styles/app.css';
import './bench.css';
import { Bench } from './Bench';
import { runParity } from './parity';

const container = document.getElementById('root');
if (container === null) throw new Error('Root element #root is missing from bench/index.html');
if (new URLSearchParams(window.location.search).get('parity') === '1') {
  // Renderer parity check (bench/parity.ts) instead of the benchmark.
  void runParity(container);
} else {
  createRoot(container).render(<Bench />);
}

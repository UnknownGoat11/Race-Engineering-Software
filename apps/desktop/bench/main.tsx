// Entry of the graph renderer benchmark page (see Bench.tsx). SYNTHETIC BENCHMARK DATA only.
import { createRoot } from 'react-dom/client';
import '../src/styles/tokens.css';
import '../src/styles/app.css';
import './bench.css';
import { Bench } from './Bench';

const container = document.getElementById('root');
if (container === null) throw new Error('Root element #root is missing from bench/index.html');
createRoot(container).render(<Bench />);

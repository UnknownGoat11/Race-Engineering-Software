import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { TimingTower } from './TimingTower';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { ScopeDqContext } from './presentation';

function makeVehicle(overrides: Partial<CompetitorEntryDto> = {}): CompetitorEntryDto {
  return {
    slot_id: 1,
    driver: 'A. Driver',
    vehicle_name: '#51 Ferrari 499P',
    vehicle_filename: null,
    class_name: 'Hypercar',
    pit_group: 'AF Corse',
    place: 1,
    class_place: 1,
    laps: 10,
    lap_dist_m: 2500,
    sector: 2,
    best_lap_s: 95.123,
    last_lap_s: 95.456,
    best_sector1_s: 31.0,
    best_sector12_s: 64.0,
    last_sector1_s: 31.2,
    last_sector12_s: 64.3,
    time_behind_next_s: 0,
    laps_behind_next: 0,
    time_behind_leader_s: 0,
    laps_behind_leader: 0,
    in_pits: false,
    in_garage_stall: false,
    pit_state: 'NONE',
    num_pitstops: 1,
    is_player: false,
    pos_x_m: 100,
    pos_z_m: 200,
    ...overrides,
  };
}

function makeScoring(vehicles: CompetitorEntryDto[], sessionType = 'Race'): ScoringStateDto {
  return {
    session: {
      track_name: 'Sebring',
      session_type: sessionType,
      session_time_remaining_s: 3600,
      end_time_s: 7200,
      lap_limit: 100,
      time_limit_s: 7200,
      yellow_flag_state: 'NONE',
    },
    vehicles,
  };
}

describe('TimingTower Component', () => {
  it('renders car numbers extracted from various vehicle name formats', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, vehicle_name: '#51 Ferrari 499P' });
    const v2 = makeVehicle({ slot_id: 2, place: 2, vehicle_name: 'Oreca 07 #81' });
    const v3 = makeVehicle({ slot_id: 3, place: 3, vehicle_name: '#92' });
    const v4 = makeVehicle({ slot_id: 4, place: 4, vehicle_name: '44' });
    const v5 = makeVehicle({ slot_id: 5, place: 5, vehicle_name: null });

    render(<TimingTower scoring={makeScoring([v1, v2, v3, v4, v5])} />);

    expect(screen.getByTestId('standings-row-1').querySelector('.standings-col-num')?.textContent).toBe('51');
    expect(screen.getByTestId('standings-row-2').querySelector('.standings-col-num')?.textContent).toBe('81');
    expect(screen.getByTestId('standings-row-3').querySelector('.standings-col-num')?.textContent).toBe('92');
    expect(screen.getByTestId('standings-row-4').querySelector('.standings-col-num')?.textContent).toBe('44');
    expect(screen.getByTestId('standings-row-5').querySelector('.standings-col-num')?.textContent).toBe('—');
  });

  it('renders leader with LDR in Gap and Int (C9)', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, driver: 'A. Leader' });
    const v2 = makeVehicle({ slot_id: 2, place: 2, driver: 'B. Chaser', time_behind_leader_s: 2.1, time_behind_next_s: 2.1 });
    render(<TimingTower scoring={makeScoring([v1, v2])} />);

    expect(screen.getByTestId('timing-tower')).toBeInTheDocument();
    const rows = screen.getAllByRole('row');
    // Header is row 0; leader is row 1
    const leaderCells = rows[1]?.querySelectorAll('td');
    // P, CLS, #, DRIVER, INT, GAP, LAST, PIT
    expect(leaderCells?.[4]?.textContent).toBe('LDR');
    expect(leaderCells?.[5]?.textContent).toBe('LDR');

    const chaserCells = rows[2]?.querySelectorAll('td');
    expect(chaserCells?.[4]?.textContent).toBe('+2.100');
    expect(chaserCells?.[5]?.textContent).toBe('+2.100');
  });

  it('computes lap gaps correctly in race', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1 });
    const v2 = makeVehicle({ slot_id: 2, place: 2, laps_behind_leader: 1, laps_behind_next: 1 });
    render(<TimingTower scoring={makeScoring([v1, v2])} />);

    const chaserCells = screen.getByTestId('standings-row-2').querySelectorAll('td');
    // v1.3 V05: lap-down reads "+1L" so it fits the fixed 8ch Gap/Int cells
    expect(chaserCells[4]?.textContent).toBe('+1L');
    expect(chaserCells[5]?.textContent).toBe('+1L');
  });

  it('formats multi-lap deficits compactly and keeps missing laps as em dash', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1 });
    const v2 = makeVehicle({ slot_id: 2, place: 2, laps_behind_leader: 3, laps_behind_next: 2 });
    const v3 = makeVehicle({
      slot_id: 3,
      place: 3,
      laps_behind_leader: null,
      laps_behind_next: null,
      time_behind_leader_s: null,
      time_behind_next_s: null,
    });
    render(<TimingTower scoring={makeScoring([v1, v2, v3])} />);

    const c2 = screen.getByTestId('standings-row-2').querySelectorAll('td');
    expect(c2[4]?.textContent).toBe('+2L');
    expect(c2[5]?.textContent).toBe('+3L');
    const c3 = screen.getByTestId('standings-row-3').querySelectorAll('td');
    expect(c3[4]?.textContent).toBe('—');
    expect(c3[5]?.textContent).toBe('—');
  });

  it('uses "+1L" for class-filtered lap deficits too', () => {
    const h1 = makeVehicle({ slot_id: 1, place: 1, class_place: 1, laps: 12, class_name: 'Hypercar' });
    const g1 = makeVehicle({ slot_id: 2, place: 2, class_place: 1, laps: 11, class_name: 'LMGT3', time_behind_leader_s: 10 });
    const g2 = makeVehicle({ slot_id: 3, place: 3, class_place: 2, laps: 10, class_name: 'LMGT3', time_behind_leader_s: 20 });
    render(<TimingTower scoring={makeScoring([h1, g1, g2])} />);
    fireEvent.click(screen.getByRole('button', { name: 'GT3' }));

    const cells = screen.getByTestId('standings-row-3').querySelectorAll('td');
    expect(cells[4]?.textContent).toBe('+1L');
    expect(cells[5]?.textContent).toBe('+1L');
  });

  it('calculates best-lap gaps in qualifying session (T23)', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, best_lap_s: 90.0 });
    const v2 = makeVehicle({ slot_id: 2, place: 2, best_lap_s: 90.35 });
    const v3 = makeVehicle({ slot_id: 3, place: 3, best_lap_s: 90.85 });

    render(<TimingTower scoring={makeScoring([v1, v2, v3], 'Qualifying')} />);

    const r1Cells = screen.getByTestId('standings-row-1').querySelectorAll('td');
    expect(r1Cells[4]?.textContent).toBe('LDR');
    expect(r1Cells[5]?.textContent).toBe('LDR');

    const r2Cells = screen.getByTestId('standings-row-2').querySelectorAll('td');
    expect(r2Cells[4]?.textContent).toBe('+0.350');
    expect(r2Cells[5]?.textContent).toBe('+0.350');

    const r3Cells = screen.getByTestId('standings-row-3').querySelectorAll('td');
    expect(r3Cells[4]?.textContent).toBe('+0.500');
    expect(r3Cells[5]?.textContent).toBe('+0.850');
  });

  it('marks the player row by row class only: no DRIVER chip and NEVER YOU (EN13, v1.3 V05)', () => {
    const player = makeVehicle({ slot_id: 5, place: 3, driver: 'Player One', is_player: true });
    render(<TimingTower scoring={makeScoring([player])} />);

    expect(screen.queryByTestId('driver-pill')).toBeNull();
    expect(screen.getByTestId('standings-row-5').querySelector('.standings-driver-name')?.textContent).toBe('Player One');
    expect(screen.queryByText('YOU')).toBeNull();
    expect(screen.getByTestId('standings-row-5')).toHaveClass('standings-row-driver');
  });

  it('formats unknown / null pitstops as em dash, never 0 (T03)', () => {
    const v = makeVehicle({ slot_id: 1, num_pitstops: null });
    render(<TimingTower scoring={makeScoring([v])} />);

    const row = screen.getByTestId('standings-row-1');
    const pitCell = row.querySelector('.standings-col-pit');
    expect(pitCell?.textContent).toBe('—');
  });

  it('renders PIT and GAR badges when car is in pit or garage stall', () => {
    const vPit = makeVehicle({ slot_id: 1, in_pits: true });
    const vGar = makeVehicle({ slot_id: 2, in_garage_stall: true });
    render(<TimingTower scoring={makeScoring([vPit, vGar])} />);

    expect(screen.getAllByText('PIT').length).toBeGreaterThan(0);
    expect(screen.getAllByText('GAR').length).toBeGreaterThan(0);
  });

  it('handles class filtering and displays class position in P column (C8, T22)', () => {
    const hy = makeVehicle({ slot_id: 1, place: 1, class_place: 1, class_name: 'Hypercar', driver: 'H. Driver' });
    const gt1 = makeVehicle({ slot_id: 2, place: 5, class_place: 1, class_name: 'GT3', driver: 'GT First', time_behind_leader_s: 30.0 });
    const gt2 = makeVehicle({ slot_id: 3, place: 6, class_place: 2, class_name: 'GT3', driver: 'GT Second', time_behind_leader_s: 32.5 });

    render(<TimingTower scoring={makeScoring([hy, gt1, gt2])} />);

    // Click GT3 filter button (code 'GT3')
    const gtBtn = screen.getByRole('button', { name: 'GT3' });
    fireEvent.click(gtBtn);

    // Hypercar row should be filtered out
    expect(screen.queryByText('H. Driver')).toBeNull();
    expect(screen.getByText('GT First')).toBeInTheDocument();
    expect(screen.getByText('GT Second')).toBeInTheDocument();

    // GT First is class leader: position in class is 1, Gap = LDR, Int = LDR
    const gt1Row = screen.getByTestId('standings-row-2');
    const gt1Cells = gt1Row.querySelectorAll('td');
    expect(gt1Cells[0]?.textContent).toBe('1');
    expect(gt1Cells[4]?.textContent).toBe('LDR');
    expect(gt1Cells[5]?.textContent).toBe('LDR');

    // GT Second: class position is 2, Gap = +2.500
    const gt2Row = screen.getByTestId('standings-row-3');
    const gt2Cells = gt2Row.querySelectorAll('td');
    expect(gt2Cells[0]?.textContent).toBe('2');
    expect(gt2Cells[4]?.textContent).toBe('+2.500');
    expect(gt2Cells[5]?.textContent).toBe('+2.500');
  });

  it('renders STALE header badge and applies stale row styling when dq is stale (T07)', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="stale" />);

    expect(screen.getByTestId('tower-stale-badge')).toHaveTextContent('STALE');
    expect(screen.getByTestId('standings-row-1')).toHaveClass('standings-row--stale');
  });

  it('renders STALE header badge from ScopeDqContext', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(
      <ScopeDqContext.Provider value="stale">
        <TimingTower scoring={makeScoring([v])} />
      </ScopeDqContext.Provider>,
    );

    expect(screen.getByTestId('tower-stale-badge')).toHaveTextContent('STALE');
  });

  it('renders ENDED header badge and applies ended table styling (T06)', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="ended" />);

    expect(screen.getByTestId('tower-ended-badge')).toHaveTextContent('ENDED');
  });

  it('renders UNTRUSTED header badge when dq is untrusted', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="untrusted" />);

    expect(screen.getByTestId('tower-untrusted-badge')).toHaveTextContent('UNTRUSTED');
  });

  it('renders class chip with tokenized class color and code via classes.ts (DS02)', () => {
    const hy = makeVehicle({ slot_id: 1, class_name: 'Hypercar' });
    const lmp = makeVehicle({ slot_id: 2, class_name: 'LMP2' });
    const gt = makeVehicle({ slot_id: 3, class_name: 'LMGT3' });

    render(<TimingTower scoring={makeScoring([hy, lmp, gt])} />);

    const row1 = screen.getByTestId('standings-row-1');
    expect(row1.querySelector('.class-chip')).toHaveTextContent('HY');

    const row2 = screen.getByTestId('standings-row-2');
    expect(row2.querySelector('.class-chip')).toHaveTextContent('P2');

    const row3 = screen.getByTestId('standings-row-3');
    expect(row3.querySelector('.class-chip')).toHaveTextContent('GT3');
  });

  it('fires onSelectSlot callback when a row is clicked', () => {
    const onSelect = vi.fn();
    const v = makeVehicle({ slot_id: 7 });
    render(<TimingTower scoring={makeScoring([v])} onSelectSlot={onSelect} />);

    fireEvent.click(screen.getByTestId('standings-row-7'));
    expect(onSelect).toHaveBeenCalledWith(7);
  });

  it('applies selected row class when selectedSlotId matches', () => {
    const v1 = makeVehicle({ slot_id: 1 });
    const v2 = makeVehicle({ slot_id: 2 });
    render(<TimingTower scoring={makeScoring([v1, v2])} selectedSlotId={2} />);

    expect(screen.getByTestId('standings-row-2')).toHaveClass('standings-row-selected');
    expect(screen.getByTestId('standings-row-1')).not.toHaveClass('standings-row-selected');
  });

  it('renders awaiting data message when scoring is empty or null', () => {
    const { rerender } = render(<TimingTower scoring={null} />);
    expect(screen.getByText('Awaiting live scoring data...')).toBeInTheDocument();

    rerender(<TimingTower scoring={makeScoring([])} />);
    expect(screen.getByText('Awaiting live scoring data...')).toBeInTheDocument();
  });

  describe('W3.3 Column sets per variant', () => {
    it('renders all 8 columns in full variant (1920)', () => {
      const v = makeVehicle({ slot_id: 1, last_lap_s: 98.5, num_pitstops: 2 });
      render(<TimingTower scoring={makeScoring([v])} variant="full" />);

      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', 'CLS', '#', 'DRIVER', 'INT', 'GAP', 'LAST', 'PIT']);

      const row = screen.getByTestId('standings-row-1');
      expect(row.querySelector('.standings-col-last')).toBeInTheDocument();
      expect(row.querySelector('.standings-col-pit')).toBeInTheDocument();
      expect(row.querySelector('.standings-col-last')?.textContent).toBe('1:38.500');
      expect(row.querySelector('.standings-col-pit')?.textContent).toBe('2');
    });

    it('drops Int, Last and Pit columns but keeps Gap in compact variant (1366, v1.3 U16)', () => {
      const v = makeVehicle({ slot_id: 1, last_lap_s: 98.5, num_pitstops: 2 });
      render(<TimingTower scoring={makeScoring([v])} variant="compact" />);

      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', 'CLS', '#', 'DRIVER', 'GAP']);

      const row = screen.getByTestId('standings-row-1');
      expect(row.querySelector('.standings-col-int')).toBeNull();
      expect(row.querySelector('.standings-col-last')).toBeNull();
      expect(row.querySelector('.standings-col-pit')).toBeNull();
      expect(row.querySelector('.standings-col-gap')?.textContent).toBe('LDR');
    });

    it('keeps the pit badge in its own cell, never inside the Gap/Int text', () => {
      const v1 = makeVehicle({ slot_id: 1, place: 1 });
      const v2 = makeVehicle({ slot_id: 2, place: 2, in_pits: true, laps_behind_leader: 1, laps_behind_next: 1 });
      render(<TimingTower scoring={makeScoring([v1, v2])} />);

      const row = screen.getByTestId('standings-row-2');
      for (const gap of row.querySelectorAll('.standings-col-gap')) {
        expect(gap.textContent).toBe('+1L');
      }
      expect(row.querySelector('.standings-col-pit')?.textContent).toBe('PIT');
      expect(row.querySelector('.standings-col-driver .pit-badge--inline')?.textContent).toBe('PIT');
    });

    it('renders 56px rail mode with only P and # columns (rail)', () => {
      const v1 = makeVehicle({ slot_id: 1, place: 1, vehicle_name: '#51 Ferrari' });
      const v2 = makeVehicle({ slot_id: 2, place: 2, vehicle_name: '#83 Ferrari' });
      render(<TimingTower scoring={makeScoring([v1, v2])} variant="rail" />);

      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', '#']);

      const row = screen.getByTestId('standings-row-1');
      const cells = row.querySelectorAll('td');
      expect(cells).toHaveLength(2);
      expect(cells[0]?.textContent).toBe('1');
      expect(cells[1]?.textContent).toBe('51');
    });

    it('expands rail mode on click or Enter, and collapses on Escape or collapse button', () => {
      const v = makeVehicle({ slot_id: 1, driver: 'A. Pier Guidi' });
      render(<TimingTower scoring={makeScoring([v])} variant="rail" />);

      // Initially in collapsed rail mode
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      expect(screen.queryByTestId('timing-tower-rail-expanded')).toBeNull();

      // Click rail to expand
      fireEvent.click(screen.getByTestId('timing-tower-rail'));
      expect(screen.getByTestId('timing-tower-rail-expanded')).toBeInTheDocument();
      expect(screen.getByText('A. Pier Guidi')).toBeInTheDocument();

      // Press Escape to collapse
      fireEvent.keyDown(window, { key: 'Escape' });
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      expect(screen.queryByTestId('timing-tower-rail-expanded')).toBeNull();

      // Expand again using Enter key
      fireEvent.keyDown(screen.getByTestId('timing-tower-rail'), { key: 'Enter' });
      expect(screen.getByTestId('timing-tower-rail-expanded')).toBeInTheDocument();

      // Collapse using collapse button
      const collapseBtn = screen.getByRole('button', { name: 'Collapse tower' });
      fireEvent.click(collapseBtn);
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
    });
  });

  describe('W3.3 Own-row pinning mechanism', () => {
    it('pins driver row at the top when driver is scrolled above viewport (pinnedOverride="top")', () => {
      const p1 = makeVehicle({ slot_id: 1, place: 1, driver: 'Leader' });
      const player = makeVehicle({ slot_id: 2, place: 2, driver: 'P. Driver', is_player: true });
      const onSelect = vi.fn();

      render(
        <TimingTower
          scoring={makeScoring([p1, player])}
          pinnedOverride="top"
          onSelectSlot={onSelect}
        />,
      );

      const pinnedRow = screen.getByTestId('standings-pinned-driver-top');
      expect(pinnedRow).toBeInTheDocument();
      expect(pinnedRow).toHaveAttribute('data-pinned', 'top');
      expect(pinnedRow).toHaveClass('standings-row-pinned--top');
      expect(pinnedRow.querySelector('.standings-driver-name')?.textContent).toBe('P. Driver');
      expect(pinnedRow.querySelector('.driver-pill')).toBeNull();
      expect(pinnedRow.textContent).not.toContain('📌');

      // Clicking pinned row triggers slot selection
      fireEvent.click(pinnedRow);
      expect(onSelect).toHaveBeenCalledWith(2);
    });

    it('pins driver row at the bottom when driver is scrolled below viewport (pinnedOverride="bottom")', () => {
      const p1 = makeVehicle({ slot_id: 1, place: 1, driver: 'Leader' });
      const player = makeVehicle({ slot_id: 2, place: 20, driver: 'P. Driver', is_player: true });

      render(
        <TimingTower
          scoring={makeScoring([p1, player])}
          pinnedOverride="bottom"
        />,
      );

      const pinnedRow = screen.getByTestId('standings-pinned-driver-bottom');
      expect(pinnedRow).toBeInTheDocument();
      expect(pinnedRow).toHaveAttribute('data-pinned', 'bottom');
      expect(pinnedRow).toHaveClass('standings-row-pinned--bottom');
      expect(pinnedRow.querySelector('.standings-driver-name')?.textContent).toBe('P. Driver');
    });

    it('does not render pinned row when driver is within viewport (pinnedOverride="none")', () => {
      const player = makeVehicle({ slot_id: 1, place: 1, driver: 'P. Driver', is_player: true });

      render(
        <TimingTower
          scoring={makeScoring([player])}
          pinnedOverride="none"
        />,
      );

      expect(screen.queryByTestId('standings-pinned-driver-top')).toBeNull();
      expect(screen.queryByTestId('standings-pinned-driver-bottom')).toBeNull();
      // Regular row is still present in tbody
      expect(screen.getByTestId('standings-row-1')).toBeInTheDocument();
    });

    it('computes pinning dynamically on scroll events', () => {
      // Create a 20-car grid where player is at position 15
      const vehicles = Array.from({ length: 20 }, (_, i) =>
        makeVehicle({
          slot_id: i + 1,
          place: i + 1,
          driver: `Driver ${i + 1}`,
          is_player: i === 14, // P15
        }),
      );

      const { container } = render(<TimingTower scoring={makeScoring(vehicles)} />);
      const tableContainer = container.querySelector('.standings-table-container');
      expect(tableContainer).not.toBeNull();

      if (tableContainer) {
        // Mock scrollHeight = 800, clientHeight = 200, scrollTop = 0 (player at P15 is below viewport)
        Object.defineProperty(tableContainer, 'scrollHeight', { value: 800, configurable: true });
        Object.defineProperty(tableContainer, 'clientHeight', { value: 200, configurable: true });
        Object.defineProperty(tableContainer, 'scrollTop', { value: 0, configurable: true, writable: true });

        const playerRow = screen.getByTestId('standings-row-15');
        Object.defineProperty(playerRow, 'offsetTop', { value: 330, configurable: true, writable: true });
        Object.defineProperty(playerRow, 'offsetHeight', { value: 22, configurable: true });

        fireEvent.scroll(tableContainer);
        expect(screen.getByTestId('standings-pinned-driver-bottom')).toBeInTheDocument();

        // Scroll down so player is in view (scrollTop = 250, rowTop = 330 is within [250, 450])
        tableContainer.scrollTop = 250;
        fireEvent.scroll(tableContainer);
        expect(screen.queryByTestId('standings-pinned-driver-bottom')).toBeNull();
        expect(screen.queryByTestId('standings-pinned-driver-top')).toBeNull();

        // Scroll further down so player is scrolled above viewport (scrollTop = 400)
        tableContainer.scrollTop = 400;
        fireEvent.scroll(tableContainer);
        expect(screen.getByTestId('standings-pinned-driver-top')).toBeInTheDocument();
      }
    });
  });

  describe('W3.3 Class filter segmented chips', () => {
    it('sets aria-pressed correctly on segmented filter buttons', () => {
      const hy = makeVehicle({ slot_id: 1, class_name: 'Hypercar' });
      const lmp = makeVehicle({ slot_id: 2, class_name: 'LMP2' });

      render(<TimingTower scoring={makeScoring([hy, lmp])} />);

      const allBtn = screen.getByRole('button', { name: 'ALL' });
      const hyBtn = screen.getByRole('button', { name: 'HY' });
      const p2Btn = screen.getByRole('button', { name: 'P2' });

      expect(allBtn).toHaveAttribute('aria-pressed', 'true');
      expect(hyBtn).toHaveAttribute('aria-pressed', 'false');
      expect(p2Btn).toHaveAttribute('aria-pressed', 'false');

      fireEvent.click(hyBtn);
      expect(allBtn).toHaveAttribute('aria-pressed', 'false');
      expect(hyBtn).toHaveAttribute('aria-pressed', 'true');
      expect(p2Btn).toHaveAttribute('aria-pressed', 'false');
    });
  });

  describe('W4.3 Keyboard accessibility and abbreviations', () => {
    it('renders accessible <abbr> elements in table header (A07)', () => {
      const v1 = makeVehicle({ slot_id: 1, place: 1 });
      render(<TimingTower scoring={makeScoring([v1])} />);

      const abbrPos = screen.getByTitle('Position');
      expect(abbrPos.tagName.toLowerCase()).toBe('abbr');
      expect(abbrPos.textContent).toBe('P');

      const abbrClass = screen.getByTitle('Class');
      expect(abbrClass.tagName.toLowerCase()).toBe('abbr');
      expect(abbrClass.textContent).toBe('CLS');

      const abbrNum = screen.getByTitle('Car Number');
      expect(abbrNum.tagName.toLowerCase()).toBe('abbr');
      expect(abbrNum.textContent).toBe('#');

      const abbrInt = screen.getByTitle('Interval to car ahead');
      expect(abbrInt.tagName.toLowerCase()).toBe('abbr');
      expect(abbrInt.textContent).toBe('INT');

      const abbrGap = screen.getByTitle('Gap to leader');
      expect(abbrGap.tagName.toLowerCase()).toBe('abbr');
      expect(abbrGap.textContent).toBe('GAP');

      const abbrLast = screen.getByTitle('Last lap time');
      expect(abbrLast.tagName.toLowerCase()).toBe('abbr');
      expect(abbrLast.textContent).toBe('LAST');

      const abbrPit = screen.getByTitle('Pit status');
      expect(abbrPit.tagName.toLowerCase()).toBe('abbr');
      expect(abbrPit.textContent).toBe('PIT');
    });

    it('renders <abbr> in rail mode header and table', () => {
      const v1 = makeVehicle({ slot_id: 1, place: 1 });
      render(<TimingTower scoring={makeScoring([v1])} variant="rail" />);

      const abbrPosList = screen.getAllByTitle('Position');
      expect(abbrPosList.length).toBe(2);
      expect(abbrPosList[0]?.textContent).toBe('POS');
      expect(abbrPosList[1]?.textContent).toBe('P');

      const abbrNum = screen.getByTitle('Car Number');
      expect(abbrNum.tagName.toLowerCase()).toBe('abbr');
      expect(abbrNum.textContent).toBe('#');
    });

    it('supports roving tabIndex and keyboard selection with Enter and Space (A03)', () => {
      const v1 = makeVehicle({ slot_id: 10, place: 1, is_player: false });
      const v2 = makeVehicle({ slot_id: 20, place: 2, is_player: true });
      const v3 = makeVehicle({ slot_id: 30, place: 3, is_player: false });
      const onSelect = vi.fn();

      const { rerender } = render(
        <TimingTower scoring={makeScoring([v1, v2, v3])} selectedSlotId={null} onSelectSlot={onSelect} />,
      );

      const row1 = screen.getByTestId('standings-row-10');
      const row2 = screen.getByTestId('standings-row-20');
      const row3 = screen.getByTestId('standings-row-30');

      // Player row (row 2) defaults to active roving tabindex (tabIndex=0) when unselected
      expect(row2).toHaveAttribute('tabindex', '0');
      expect(row1).toHaveAttribute('tabindex', '-1');
      expect(row3).toHaveAttribute('tabindex', '-1');
      expect(row1).toHaveAttribute('aria-selected', 'false');
      expect(row2).toHaveAttribute('aria-selected', 'false');

      // ArrowDown navigates to row 3
      fireEvent.keyDown(row2, { key: 'ArrowDown' });
      expect(row3).toHaveAttribute('tabindex', '0');
      expect(row2).toHaveAttribute('tabindex', '-1');

      // ArrowUp navigates back to row 2
      fireEvent.keyDown(row3, { key: 'ArrowUp' });
      expect(row2).toHaveAttribute('tabindex', '0');

      // Home jumps to first row (row 1)
      fireEvent.keyDown(row2, { key: 'Home' });
      expect(row1).toHaveAttribute('tabindex', '0');

      // End jumps to last row (row 3)
      fireEvent.keyDown(row1, { key: 'End' });
      expect(row3).toHaveAttribute('tabindex', '0');

      // Enter key selects row 3
      fireEvent.keyDown(row3, { key: 'Enter' });
      expect(onSelect).toHaveBeenCalledWith(30);

      // Space key selects row 1
      fireEvent.keyDown(row1, { key: ' ' });
      expect(onSelect).toHaveBeenCalledWith(10);

      // With selectedSlotId=30, row 3 has aria-selected=true
      rerender(
        <TimingTower scoring={makeScoring([v1, v2, v3])} selectedSlotId={30} onSelectSlot={onSelect} />,
      );
      expect(screen.getByTestId('standings-row-30')).toHaveAttribute('aria-selected', 'true');
      expect(screen.getByTestId('standings-row-10')).toHaveAttribute('aria-selected', 'false');
    });

    it('supports keyboard navigation in rail mode', () => {
      const v1 = makeVehicle({ slot_id: 10, place: 1 });
      const v2 = makeVehicle({ slot_id: 20, place: 2 });
      const onSelect = vi.fn();

      render(
        <TimingTower scoring={makeScoring([v1, v2])} variant="rail" onSelectSlot={onSelect} />,
      );

      const row1 = screen.getByTestId('standings-row-10');
      const row2 = screen.getByTestId('standings-row-20');

      expect(row1).toHaveAttribute('tabindex', '0');
      expect(row2).toHaveAttribute('tabindex', '-1');

      fireEvent.keyDown(row1, { key: 'ArrowDown' });
      expect(row2).toHaveAttribute('tabindex', '0');

      fireEvent.keyDown(row2, { key: 'Enter' });
      expect(onSelect).toHaveBeenCalledWith(20);
    });
  });
});

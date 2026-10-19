import { useState, type JSX } from 'react';
import type { ViewId } from '../views/types';
import { ALL_VIEWS, getViewDefinition } from '../views/viewRegistry';
import { ViewSwitcherDropdown } from '../views/ViewSwitcherDropdown';

// Import View Components
import { DriverDnaView } from '../views/components/DriverDnaView';
import { DrivingView } from '../views/components/DrivingView';
import { LapComparisonView } from '../views/components/LapComparisonView';
import { MulticarRadarCircleView } from '../views/components/MulticarRadarCircleView';
import { MinisectorTimingView } from '../views/components/MinisectorTimingView';
import { FieldSpreadChartView } from '../views/components/FieldSpreadChartView';
import { MultihistogramView } from '../views/components/MultihistogramView';
import { MultiplotterView } from '../views/components/MultiplotterView';
import { StintsDataView } from '../views/components/StintsDataView';
import {
  StatisticsTableView,
  StatisticsTableGraphsView,
  StatisticsRedBlueView,
} from '../views/components/StatisticsViews';
import { RawDataInspectorView } from '../views/components/RawDataInspectorView';
import { MinimalCockpitHudView } from '../views/components/MinimalCockpitHudView';
import {
  LaptimeDataView,
  RaceInfoView,
  TimeTrialInfoView,
  MulticarMapView,
  MulticarRaceTimingView,
  MulticarOneLapTimingView,
  MulticarOpponentStatsView,
  MulticarRaceHistoryView,
  MulticarSessionDataView,
  MulticarTelemetryView,
  MultilapTelemetryView,
  WorkspaceView,
} from '../views/components/OtherViews';

export interface ViewsScreenProps {
  readonly initialViewId?: ViewId;
}

export function ViewsScreen({ initialViewId = 'driving' }: ViewsScreenProps): JSX.Element {
  const [activeViewId, setActiveViewId] = useState<ViewId>(initialViewId);
  const activeDef = getViewDefinition(activeViewId);

  // Navigate to previous or next view in the catalogue
  const currentIndex = ALL_VIEWS.findIndex((v) => v.id === activeViewId);
  const handlePrev = () => {
    const prevIdx = (currentIndex - 1 + ALL_VIEWS.length) % ALL_VIEWS.length;
    const prev = ALL_VIEWS[prevIdx];
    if (prev) setActiveViewId(prev.id);
  };
  const handleNext = () => {
    const nextIdx = (currentIndex + 1) % ALL_VIEWS.length;
    const next = ALL_VIEWS[nextIdx];
    if (next) setActiveViewId(next.id);
  };

  const renderActiveView = () => {
    switch (activeViewId) {
      case 'driver-dna':
        return <DriverDnaView />;
      case 'driving':
        return <DrivingView />;
      case 'lap-comparison':
        return <LapComparisonView />;
      case 'multicar-circle':
        return <MulticarRadarCircleView />;
      case 'multicar-minisector-timing':
        return <MinisectorTimingView />;
      case 'multicar-race-field-spread':
        return <FieldSpreadChartView />;
      case 'multihistogram':
        return <MultihistogramView />;
      case 'multiplotter':
        return <MultiplotterView />;
      case 'stints-data':
        return <StintsDataView />;
      case 'statistics-table':
        return <StatisticsTableView />;
      case 'statistics-table-graphs':
        return <StatisticsTableGraphsView />;
      case 'statistics-red-blue':
        return <StatisticsRedBlueView />;
      case 'raw-data':
        return <RawDataInspectorView />;
      case 'minimal-ui':
        return <MinimalCockpitHudView />;
      case 'laptime-data':
        return <LaptimeDataView />;
      case 'race-info':
        return <RaceInfoView />;
      case 'time-trial':
        return <TimeTrialInfoView />;
      case 'multicar-map':
        return <MulticarMapView />;
      case 'multicar-race-timing':
        return <MulticarRaceTimingView />;
      case 'multicar-one-lap-timing':
        return <MulticarOneLapTimingView />;
      case 'multicar-opponent-stats':
        return <MulticarOpponentStatsView />;
      case 'multicar-race-history':
        return <MulticarRaceHistoryView />;
      case 'multicar-session-data':
        return <MulticarSessionDataView />;
      case 'multicar-telemetry':
        return <MulticarTelemetryView />;
      case 'multilap-telemetry':
        return <MultilapTelemetryView />;
      case 'workspace':
      case 'telemetry':
      default:
        return <WorkspaceView />;
    }
  };

  return (
    <div className="views-screen-container" data-testid="views-screen">
      {/* Top Views Control Bar with Dropdown Switcher */}
      <div className="views-control-bar">
        <div className="views-control-left">
          <ViewSwitcherDropdown activeViewId={activeViewId} onSelectView={setActiveViewId} />
          <div className="views-nav-buttons">
            <button
              type="button"
              className="btn btn-sm btn-icon"
              onClick={handlePrev}
              title="Previous view (Left arrow)"
              aria-label="Previous view"
            >
              ◀
            </button>
            <span className="views-index-text">
              {currentIndex + 1} / {ALL_VIEWS.length}
            </span>
            <button
              type="button"
              className="btn btn-sm btn-icon"
              onClick={handleNext}
              title="Next view (Right arrow)"
              aria-label="Next view"
            >
              ▶
            </button>
          </div>
        </div>

        <div className="views-control-right">
          <span className="views-active-title">
            {activeDef.glyph} {activeDef.title}
          </span>
          <span className="badge badge-subtle">{activeDef.category.toUpperCase()}</span>
          <span className="badge badge-mode">V1.1</span>
        </div>
      </div>

      {/* Main Viewport */}
      <div className="views-viewport" data-testid={`view-container-${activeViewId}`}>
        {renderActiveView()}
      </div>
    </div>
  );
}

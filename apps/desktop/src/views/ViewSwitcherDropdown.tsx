import { useState, useRef, useEffect, type JSX, type KeyboardEvent } from 'react';
import type { ViewCategory, ViewDefinition, ViewId } from './types';
import { ALL_VIEWS, CATEGORY_LABELS, getViewDefinition } from './viewRegistry';

export interface ViewSwitcherDropdownProps {
  readonly activeViewId: ViewId;
  readonly onSelectView: (id: ViewId) => void;
  readonly className?: string;
}

export function ViewSwitcherDropdown({
  activeViewId,
  onSelectView,
  className = '',
}: ViewSwitcherDropdownProps): JSX.Element {
  const [isOpen, setIsOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedCategory, setSelectedCategory] = useState<ViewCategory | 'all'>('all');
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const activeView = getViewDefinition(activeViewId);

  // Close on outside click
  useEffect(() => {
    function handlePointerDown(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    }
    if (isOpen) {
      document.addEventListener('mousedown', handlePointerDown);
      searchInputRef.current?.focus();
    }
    return () => {
      document.removeEventListener('mousedown', handlePointerDown);
    };
  }, [isOpen]);

  // Filter views based on category and search query
  const filteredViews = ALL_VIEWS.filter((v) => {
    if (selectedCategory !== 'all' && v.category !== selectedCategory) {
      return false;
    }
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      return (
        v.title.toLowerCase().includes(q) ||
        v.description.toLowerCase().includes(q) ||
        v.id.toLowerCase().includes(q)
      );
    }
    return true;
  });

  const handleKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      setIsOpen(false);
    }
  };

  const handleSelect = (view: ViewDefinition) => {
    onSelectView(view.id);
    setIsOpen(false);
    setSearchQuery('');
  };

  return (
    <div className={`view-switcher-dropdown ${className}`} ref={dropdownRef} onKeyDown={handleKeyDown}>
      {/* Trigger Button */}
      <button
        type="button"
        className="btn view-switcher-trigger"
        onClick={() => {
          setIsOpen(!isOpen);
        }}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        data-testid="view-switcher-button"
      >
        <span className="view-trigger-glyph">{activeView.glyph}</span>
        <span className="view-trigger-label">{activeView.title}</span>
        <span className="view-trigger-badge">V1.1</span>
        <span className="view-trigger-arrow" aria-hidden="true">{isOpen ? '▲' : '▼'}</span>
      </button>

      {/* Dropdown Menu Popup */}
      {isOpen && (
        <div className="view-dropdown-popup" role="listbox" data-testid="view-dropdown-menu">
          {/* Search Box */}
          <div className="view-search-box">
            <span className="view-search-icon" aria-hidden="true">🔍</span>
            <input
              ref={searchInputRef}
              type="text"
              className="view-search-input"
              placeholder="Search views (e.g. driving, radar, delta)..."
              value={searchQuery}
              onChange={(e) => {
                setSearchQuery(e.target.value);
              }}
              aria-label="Filter views"
              data-testid="view-search-input"
            />
            {searchQuery && (
              <button
                type="button"
                className="view-search-clear"
                onClick={() => {
                  setSearchQuery('');
                }}
                aria-label="Clear search"
              >
                ×
              </button>
            )}
          </div>

          {/* Category Filter Pills */}
          <div className="view-category-pills" role="tablist" aria-label="View categories">
            <button
              type="button"
              className={`cat-pill ${selectedCategory === 'all' ? 'cat-pill-active' : ''}`}
              onClick={() => {
                setSelectedCategory('all');
              }}
            >
              All (27)
            </button>
            <button
              type="button"
              className={`cat-pill ${selectedCategory === 'performance' ? 'cat-pill-active' : ''}`}
              onClick={() => {
                setSelectedCategory('performance');
              }}
            >
              {CATEGORY_LABELS.performance.glyph} Performance
            </button>
            <button
              type="button"
              className={`cat-pill ${selectedCategory === 'timing' ? 'cat-pill-active' : ''}`}
              onClick={() => {
                setSelectedCategory('timing');
              }}
            >
              {CATEGORY_LABELS.timing.glyph} Timing
            </button>
            <button
              type="button"
              className={`cat-pill ${selectedCategory === 'multicar' ? 'cat-pill-active' : ''}`}
              onClick={() => {
                setSelectedCategory('multicar');
              }}
            >
              {CATEGORY_LABELS.multicar.glyph} Multi-Car
            </button>
            <button
              type="button"
              className={`cat-pill ${selectedCategory === 'engineering' ? 'cat-pill-active' : ''}`}
              onClick={() => {
                setSelectedCategory('engineering');
              }}
            >
              {CATEGORY_LABELS.engineering.glyph} Engineering
            </button>
          </div>

          {/* View Items List matching the image order */}
          <div className="view-items-list">
            {filteredViews.length === 0 ? (
              <div className="view-no-results">No views found matching "{searchQuery}"</div>
            ) : (
              filteredViews.map((v) => {
                const isSelected = v.id === activeViewId;
                return (
                  <button
                    key={v.id}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className={`view-item-row ${isSelected ? 'view-item-active' : ''}`}
                    onClick={() => {
                      handleSelect(v);
                    }}
                    data-testid={`view-item-${v.id}`}
                  >
                    <div className="view-item-left">
                      <span className="view-item-glyph">{v.glyph}</span>
                      <div className="view-item-text">
                        <span className="view-item-title">{v.title}</span>
                        <span className="view-item-desc">{v.description}</span>
                      </div>
                    </div>
                    <div className="view-item-right">
                      {v.badge && <span className="badge badge-subtle">{v.badge}</span>}
                      {isSelected && <span className="view-item-check" aria-hidden="true">✓</span>}
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </div>
      )}
    </div>
  );
}

import type { Meta, StoryObj } from "@storybook/react-vite";
import type { ComponentProps } from "react";

import {
  MetricCard,
  MetricCardDetail,
  MetricCardGauge,
  MetricCardGroup,
  MetricCardLabel,
  MetricCardTrend,
  MetricCardValue,
  getMetricTrendTone,
} from "../index";

type ObservedUsagePreviewProps = {
  containerWidth: number;
  monthDetail: string;
  monthFill: number;
  monthLabel: string;
  monthTrend: string;
  monthValue: string;
  todayDetail: string;
  todayFill: number;
  todayLabel: string;
  todayTrend: string;
  todayValue: string;
  weekDetail: string;
  weekFill: number;
  weekLabel: string;
  weekTrend: string;
  weekValue: string;
};

function ObservedUsagePreview({
  containerWidth,
  monthDetail,
  monthFill,
  monthLabel,
  monthTrend,
  monthValue,
  todayDetail,
  todayFill,
  todayLabel,
  todayTrend,
  todayValue,
  weekDetail,
  weekFill,
  weekLabel,
  weekTrend,
  weekValue,
}: ObservedUsagePreviewProps) {
  const metrics = [
    {
      detail: todayDetail,
      fill: todayFill,
      label: todayLabel,
      tone: "today" as const,
      trend: todayTrend,
      value: todayValue,
    },
    {
      detail: weekDetail,
      fill: weekFill,
      label: weekLabel,
      tone: "week" as const,
      trend: weekTrend,
      value: weekValue,
    },
    {
      detail: monthDetail,
      fill: monthFill,
      label: monthLabel,
      tone: "month" as const,
      trend: monthTrend,
      value: monthValue,
    },
  ];

  return (
    <div style={{ width: containerWidth }}>
      <MetricCardGroup>
        {metrics.map((metric) => (
          <MetricCard key={metric.tone}>
            <MetricCardLabel>{metric.label}</MetricCardLabel>
            <MetricCardTrend tone={getMetricTrendTone(metric.trend)}>
              {metric.trend}
            </MetricCardTrend>
            <MetricCardValue>{metric.value}</MetricCardValue>
            <MetricCardDetail tone="positive">
              {metric.detail}
            </MetricCardDetail>
            <MetricCardGauge fill={metric.fill} tone={metric.tone} />
          </MetricCard>
        ))}
      </MetricCardGroup>
    </div>
  );
}

const fillControl = {
  control: { max: 100, min: 0, step: 1, type: "range" },
} as const;

type MetricCardStoryArgs = Omit<
  ComponentProps<typeof MetricCardGroup>,
  "children"
> &
  ObservedUsagePreviewProps;

const meta = {
  args: {
    containerWidth: 368,
    monthDetail: "≈ $856.73",
    monthFill: 100,
    monthLabel: "30 days",
    monthTrend: "+22%",
    monthValue: "284.6M",
    todayDetail: "≈ $38.61",
    todayFill: 34,
    todayLabel: "Today",
    todayTrend: "+8%",
    todayValue: "12.8M",
    weekDetail: "≈ $214.96",
    weekFill: 64,
    weekLabel: "7 days",
    weekTrend: "+14%",
    weekValue: "71.4M",
  },
  argTypes: {
    containerWidth: {
      control: { max: 520, min: 300, step: 1, type: "range" },
      table: { category: "Layout" },
    },
    monthDetail: { table: { category: "30 days" } },
    monthFill: { ...fillControl, table: { category: "30 days" } },
    monthLabel: { table: { category: "30 days" } },
    monthTrend: { table: { category: "30 days" } },
    monthValue: { table: { category: "30 days" } },
    todayDetail: { table: { category: "Today" } },
    todayFill: { ...fillControl, table: { category: "Today" } },
    todayLabel: { table: { category: "Today" } },
    todayTrend: { table: { category: "Today" } },
    todayValue: { table: { category: "Today" } },
    weekDetail: { table: { category: "7 days" } },
    weekFill: { ...fillControl, table: { category: "7 days" } },
    weekLabel: { table: { category: "7 days" } },
    weekTrend: { table: { category: "7 days" } },
    weekValue: { table: { category: "7 days" } },
  },
  component: MetricCardGroup,
  parameters: {
    docs: {
      description: {
        component:
          "The shared metric-card family used for observed-token totals, trends, API-equivalent values, and gauges.",
      },
    },
    layout: "centered",
  },
  render: (args) => <ObservedUsagePreview {...args} />,
  title: "Components/Metric cards",
} satisfies Meta<MetricCardStoryArgs>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Observed: Story = {};

export const Unavailable: Story = {
  args: {
    monthDetail: "Not observed",
    monthFill: 0,
    monthTrend: "—",
    monthValue: "—",
    todayDetail: "Not observed",
    todayFill: 0,
    todayTrend: "—",
    todayValue: "—",
    weekDetail: "Not observed",
    weekFill: 0,
    weekTrend: "—",
    weekValue: "—",
  },
};

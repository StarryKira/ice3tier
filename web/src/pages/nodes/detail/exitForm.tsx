import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Segmented } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'

import { type ExitFormState, exitFormProblems } from './exitFormState'

interface ExitFormFieldsProps {
  state: ExitFormState
  onChange: (state: ExitFormState) => void
  /** 用户点过提交之后才把错误显示出来。 */
  showProblems: boolean
}

export function ExitFormFields({ state, onChange, showProblems }: ExitFormFieldsProps) {
  const problems = exitFormProblems(state)
  const shown = (problem: string | null) => (showProblems ? problem : null)
  return (
    <div className="space-y-4">
      <Field
        label={t.nodes.detail.exitFormPorts}
        hint={state.limitPorts ? t.nodes.detail.exitFormPortsHint : undefined}
        error={shown(problems.ports)}
      >
        {(props) => (
          <div className="flex flex-wrap items-center gap-2">
            <Segmented
              aria-label={t.nodes.detail.exitFormPorts}
              size="sm"
              value={state.limitPorts ? 'limited' : 'any'}
              onChange={(value) => onChange({ ...state, limitPorts: value === 'limited' })}
              options={[
                { value: 'limited', label: t.nodes.detail.exitFormPortsLimited },
                { value: 'any', label: t.nodes.detail.exitFormPortsAny },
              ]}
            />
            {state.limitPorts ? (
              <Input
                {...props}
                className="w-52 font-mono"
                value={state.ports}
                onChange={(event) => onChange({ ...state, ports: event.target.value })}
              />
            ) : null}
          </div>
        )}
      </Field>
      <Field
        label={t.nodes.detail.exitFormBandwidth}
        hint={t.nodes.detail.exitFormBandwidthHint}
        error={shown(problems.bandwidth)}
        optional={t.common.optional}
      >
        {(props) => (
          <div className="flex items-center gap-2">
            <Input
              {...props}
              inputMode="numeric"
              className="w-28 tabular"
              value={state.bandwidth}
              onChange={(event) => onChange({ ...state, bandwidth: event.target.value })}
            />
            <span className="text-[13px] text-muted-foreground">{t.common.mbps}</span>
          </div>
        )}
      </Field>
      <label className="flex items-start gap-3">
        <Switch checked={state.lan} onCheckedChange={(lan) => onChange({ ...state, lan })} className="mt-0.5" />
        <span>
          <span className="block text-[13px] font-medium">{t.nodes.detail.exitFormLan}</span>
          <span className="block text-[13px] text-muted-foreground">{t.nodes.detail.exitFormLanHint}</span>
        </span>
      </label>
      <Field
        label={t.nodes.detail.exitFormUpstream}
        hint={t.nodes.detail.exitFormUpstreamHint}
        error={shown(problems.upstream)}
        optional={t.common.optional}
      >
        {(props) => (
          <Input
            {...props}
            className="w-64 font-mono"
            placeholder="127.0.0.1:1080"
            value={state.upstream}
            onChange={(event) => onChange({ ...state, upstream: event.target.value })}
          />
        )}
      </Field>
    </div>
  )
}

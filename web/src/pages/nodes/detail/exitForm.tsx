import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Segmented } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { BytesInput, ClassSelect } from '@/components/usage'
import { t } from '@/i18n'
import { MAX_QUOTA_RESET_DAY } from '@/lib/validate'

import { type ExitFormState, exitFormProblems } from './exitFormState'

interface ExitFormFieldsProps {
  state: ExitFormState
  onChange: (state: ExitFormState) => void
  /** 用户点过提交之后才把错误显示出来。 */
  showProblems: boolean
}

const d = t.nodes.detail

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
      <div className="space-y-4 border-t pt-4">
        <p className="text-[13px] font-semibold">{d.exitFormMore}</p>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-[minmax(0,1fr)_auto]">
          <Field
            label={d.exitFormQuota}
            hint={d.exitFormQuotaHint}
            error={shown(problems.quota)}
            optional={t.common.optional}
          >
            {(props) => (
              <BytesInput
                {...props}
                value={state.quota}
                unit={state.quotaUnit}
                unitLabel={d.exitFormQuotaUnit}
                onChange={(quota, quotaUnit) => onChange({ ...state, quota, quotaUnit })}
              />
            )}
          </Field>
          {state.quota.trim() !== '' ? (
            <Field label={d.exitFormResetDay} hint={d.exitFormResetDayHint} error={shown(problems.resetDay)}>
              {(props) => (
                <Input
                  {...props}
                  type="number"
                  min={1}
                  max={MAX_QUOTA_RESET_DAY}
                  inputMode="numeric"
                  className="w-24 tabular"
                  value={state.resetDay}
                  onChange={(event) => onChange({ ...state, resetDay: event.target.value })}
                />
              )}
            </Field>
          ) : null}
        </div>
        <Field label={d.exitFormFamily} hint={d.exitFormFamilyHint}>
          {() => (
            <Segmented
              aria-label={d.exitFormFamily}
              size="sm"
              value={state.family}
              onChange={(family) => onChange({ ...state, family })}
              options={[
                { value: 'any', label: d.exitFormFamilyAny },
                { value: 'ipv4', label: d.exitFormFamilyIpv4 },
                { value: 'ipv6', label: d.exitFormFamilyIpv6 },
              ]}
              className="w-fit"
            />
          )}
        </Field>
        <Field
          label={d.exitFormConnects}
          hint={d.exitFormConnectsHint}
          error={shown(problems.connects)}
          optional={t.common.optional}
        >
          {(props) => (
            <div className="flex items-center gap-2">
              <Input
                {...props}
                inputMode="numeric"
                className="w-28 tabular"
                placeholder={t.common.unlimited}
                value={state.connects}
                onChange={(event) => onChange({ ...state, connects: event.target.value })}
              />
              <span className="text-[13px] text-muted-foreground">{d.exitFormConnectsUnit}</span>
            </div>
          )}
        </Field>
        <Field label={d.exitFormClass} hint={d.exitFormClassHint}>
          {(props) => (
            <ClassSelect
              {...props}
              value={state.serviceClass}
              inheritLabel={d.exitFormClassNone}
              onChange={(serviceClass) => onChange({ ...state, serviceClass })}
            />
          )}
        </Field>
      </div>
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

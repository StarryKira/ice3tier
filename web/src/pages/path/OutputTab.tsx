import { Download } from 'lucide-react'
import { useState } from 'react'

import { CopyButton } from '@/components/copy'
import { Button } from '@/components/ui/button'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { Segmented } from '@/components/ui/segmented'
import { t } from '@/i18n'

const d = t.diagnosis

type Format = 'text' | 'json'

/** 把一段文字存成文件：浏览器的下载。 */
function save(name: string, content: string, type: string) {
  const url = URL.createObjectURL(new Blob([content], { type }))
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}

/**
 * 完整输出：一份文本报告（贴给别人看）和一份 JSON（原样的数据），可以复制、下载。
 * 文件名带上两个节点和时刻，存了几份也分得开。
 */
export function OutputTab({ text, json, fileBase }: { text: string; json: string; fileBase: string }) {
  const [format, setFormat] = useState<Format>('text')
  const content = format === 'text' ? text : json
  return (
    <BoxCard data-testid="diagnosis-output">
      <BoxCardHeader
        title={
          <Segmented<Format>
            size="sm"
            aria-label={d.tabs.output}
            value={format}
            onChange={setFormat}
            options={[
              { value: 'text', label: d.outputText },
              { value: 'json', label: d.outputJson },
            ]}
          />
        }
        tip={d.outputHint}
        action={
          <>
            <CopyButton value={content} size="sm">
              {d.copy}
            </CopyButton>
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                format === 'text'
                  ? save(`${fileBase}.txt`, text, 'text/plain;charset=utf-8')
                  : save(`${fileBase}.json`, json, 'application/json')
              }
            >
              <Download aria-hidden />
              {d.download}
            </Button>
          </>
        }
      />
      <pre
        className="overflow-x-auto px-5 py-4 font-mono text-xs leading-relaxed whitespace-pre text-foreground"
        data-testid={`diagnosis-output-${format}`}
      >
        {content}
      </pre>
    </BoxCard>
  )
}

/**
 * 英文的几个写法上的小工具，只给 `en/` 下的目录用。
 */

/**
 * 冒号后面接一个状态的短语（"Listener lan: bind failed"）：英文句中不该大写开头。
 * 第二个字母也是大写的缩写（"UDP …"、"IPv6 only"）保持原样。
 */
export function afterColon(label: string | number): string {
  // 也收数：目录的测试（`i18n.test.ts`）用数字当样例参数调每个函数。
  const text = String(label)
  const second = text.charAt(1)
  const acronym = second !== second.toLowerCase()
  return acronym ? text : text.charAt(0).toLowerCase() + text.slice(1)
}

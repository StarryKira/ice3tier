/**
 * 常见端口的协议名，给出口的端口限制用：端口旁边写上它通常是什么，常用的几个可以一点就加。
 * 这些是协议的名字，两种语言写法一样，不进翻译。
 */
const PORT_NAMES: Readonly<Record<number, string>> = {
  21: 'FTP',
  22: 'SSH',
  25: 'SMTP',
  53: 'DNS',
  80: 'HTTP',
  110: 'POP3',
  143: 'IMAP',
  443: 'HTTPS',
  465: 'SMTPS',
  587: 'SMTP',
  993: 'IMAPS',
  995: 'POP3S',
  3306: 'MySQL',
  5432: 'PostgreSQL',
  6379: 'Redis',
  8080: 'HTTP',
  8443: 'HTTPS',
}

/** 一点就加的那几个，按常用程度排。 */
export const COMMON_PORTS: readonly number[] = [443, 80, 22, 53, 8080, 8443]

export function portName(port: number): string | undefined {
  return PORT_NAMES[port]
}

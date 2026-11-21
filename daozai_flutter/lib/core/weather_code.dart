//! 天气现象映射 —— 忠实复刻原项目 src/weather.rs 的 kind() 与 describe()。

/// 天气现象；决定画太阳、云、雨还是雪。
enum WeatherKind {
  sun, moon, partly, partlyNight, cloud, fog,
  drizzle, rain, shower, snow, thunder,
}

/// WMO 天气代码 → 图标类型（day 决定太阳还是月亮）。
WeatherKind weatherKind(int code, bool day) {
  switch (code) {
    case 0:
      return day ? WeatherKind.sun : WeatherKind.moon;
    case 1:
    case 2:
      return day ? WeatherKind.partly : WeatherKind.partlyNight;
    case 3:
      return WeatherKind.cloud;
    case 45:
    case 48:
      return WeatherKind.fog;
    case 51:
    case 53:
    case 55:
    case 56:
    case 57:
      return WeatherKind.drizzle;
    case 61:
    case 63:
    case 65:
    case 66:
    case 67:
      return WeatherKind.rain;
    case 71:
    case 73:
    case 75:
    case 77:
    case 85:
    case 86:
      return WeatherKind.snow;
    case 80:
    case 81:
    case 82:
      return WeatherKind.shower;
    case 95:
    case 96:
    case 97:
    case 99:
      return WeatherKind.thunder;
    default:
      return WeatherKind.cloud;
  }
}

/// WMO 天气代码 → 中文描述。
String weatherDescribe(int code) {
  switch (code) {
    case 0: return '晴';
    case 1: return '晴间多云';
    case 2: return '多云';
    case 3: return '阴';
    case 45:
    case 48: return '雾';
    case 51:
    case 53: return '毛毛雨';
    case 55: return '强毛毛雨';
    case 56:
    case 57:
    case 66:
    case 67: return '冻雨';
    case 61: return '小雨';
    case 63: return '中雨';
    case 65: return '大雨';
    case 71: return '小雪';
    case 73: return '中雪';
    case 75: return '大雪';
    case 77: return '米雪';
    case 80: return '阵雨';
    case 81: return '强阵雨';
    case 82: return '暴雨';
    case 85: return '阵雪';
    case 86: return '强阵雪';
    case 95: return '雷阵雨';
    case 97: return '强雷阵雨';
    case 96:
    case 99: return '雷暴伴冰雹';
    default: return '多云';
  }
}

/// 天气图标（Material Icons 近似；原版为矢量绘制，骨架阶段用图标占位）。
String weatherEmoji(WeatherKind kind) {
  switch (kind) {
    case WeatherKind.sun: return '☀️';
    case WeatherKind.moon: return '🌙';
    case WeatherKind.partly: return '⛅';
    case WeatherKind.partlyNight: return '☁️';
    case WeatherKind.cloud: return '☁️';
    case WeatherKind.fog: return '🌫️';
    case WeatherKind.drizzle: return '🌦️';
    case WeatherKind.rain: return '🌧️';
    case WeatherKind.shower: return '🌧️';
    case WeatherKind.snow: return '🌨️';
    case WeatherKind.thunder: return '⛈️';
  }
}
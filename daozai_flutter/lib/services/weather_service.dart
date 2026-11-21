//! 天气服务 —— 复刻原项目 src/weather.rs 的 Open-Meteo 请求与解析。
//! 免密钥、无需账号；结果缓存在本机，断网时继续显示上次数据。
import 'dart:convert';
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';
import '../core/weather_code.dart';

class WeatherDay {
  final String label;
  final String date;
  final double high;
  final double low;
  final int code;
  final int rain;
  const WeatherDay({
    required this.label,
    required this.date,
    required this.high,
    required this.low,
    required this.code,
    required this.rain,
  });

  Map<String, dynamic> toJson() =>
      {'label': label, 'date': date, 'high': high, 'low': low, 'code': code, 'rain': rain};

  factory WeatherDay.fromJson(Map<String, dynamic> j) => WeatherDay(
        label: j['label'] as String? ?? '',
        date: j['date'] as String? ?? '',
        high: (j['high'] as num?)?.toDouble() ?? 0,
        low: (j['low'] as num?)?.toDouble() ?? 0,
        code: (j['code'] as num?)?.toInt() ?? 3,
        rain: (j['rain'] as num?)?.toInt() ?? 0,
      );
}

class WeatherSnapshot {
  final String place;
  final double temp;
  final double feelsLike;
  final int humidity;
  final double wind;
  final int code;
  final bool isDay;
  final String? sunrise;
  final String? sunset;
  final List<WeatherDay> days;
  final int at; // Unix 秒

  const WeatherSnapshot({
    required this.place,
    required this.temp,
    required this.feelsLike,
    required this.humidity,
    required this.wind,
    required this.code,
    required this.isDay,
    this.sunrise,
    this.sunset,
    required this.days,
    required this.at,
  });

  WeatherKind get kind => weatherKind(code, isDay);
  String get describe => weatherDescribe(code);

  Map<String, dynamic> toJson() => {
        'place': place, 'temp': temp, 'feelsLike': feelsLike, 'humidity': humidity,
        'wind': wind, 'code': code, 'isDay': isDay, 'sunrise': sunrise, 'sunset': sunset,
        'days': days.map((d) => d.toJson()).toList(), 'at': at,
      };

  factory WeatherSnapshot.fromJson(Map<String, dynamic> j) => WeatherSnapshot(
        place: j['place'] as String? ?? '',
        temp: (j['temp'] as num?)?.toDouble() ?? 0,
        feelsLike: (j['feelsLike'] as num?)?.toDouble() ?? 0,
        humidity: (j['humidity'] as num?)?.toInt() ?? 0,
        wind: (j['wind'] as num?)?.toDouble() ?? 0,
        code: (j['code'] as num?)?.toInt() ?? 3,
        isDay: j['isDay'] as bool? ?? true,
        sunrise: j['sunrise'] as String?,
        sunset: j['sunset'] as String?,
        days: (j['days'] as List? ?? [])
            .map((d) => WeatherDay.fromJson(d as Map<String, dynamic>))
            .toList(),
        at: (j['at'] as num?)?.toInt() ?? 0,
      );
}

class GeoPosition {
  final double lat;
  final double lon;
  final String label;
  final String region;
  const GeoPosition(this.lat, this.lon, this.label, this.region);
}

class WeatherService {
  static const int intervalSecs = 900; // 15 分钟
  static const int forecastDays = 7;
  static const String _cacheKey = 'daozai.weather.cache';

  final http.Client _client = http.Client();

  /// 按城市名搜索坐标（Open-Meteo Geocoding，中文）。
  Future<GeoPosition?> geocode(String city) async {
    final uri = Uri.https('geocoding-api.open-meteo.com', '/v1/search', {
      'name': city, 'count': '1', 'language': 'zh', 'format': 'json',
    });
    final res = await _client.get(uri).timeout(const Duration(seconds: 12));
    if (res.statusCode != 200) return null;
    final results = (jsonDecode(res.body)['results'] as List?);
    if (results == null || results.isEmpty) return null;
    final item = results.first as Map<String, dynamic>;
    final areas = <String>[];
    for (final key in ['admin2', 'admin1', 'country']) {
      final v = item[key] as String?;
      if (v != null && v.isNotEmpty && v != item['name'] && !areas.contains(v)) areas.add(v);
    }
    return GeoPosition(
      (item['latitude'] as num).toDouble(),
      (item['longitude'] as num).toDouble(),
      (item['name'] as String?)?.trim() ?? city,
      areas.join(' · '),
    );
  }

  /// 拉取实况 + 七日预报。
  Future<WeatherSnapshot> fetch(GeoPosition pos) async {
    final uri = Uri.https('api.open-meteo.com', '/v1/forecast', {
      'latitude': pos.lat.toStringAsFixed(4),
      'longitude': pos.lon.toStringAsFixed(4),
      'current': 'temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m',
      'daily': 'weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,sunrise,sunset',
      'timezone': 'auto',
      'forecast_days': '$forecastDays',
    });
    final res = await _client.get(uri).timeout(const Duration(seconds: 15));
    if (res.statusCode != 200) {
      throw Exception('天气服务返回 ${res.statusCode}');
    }
    return _parse(pos.label, jsonDecode(res.body) as Map<String, dynamic>);
  }

  WeatherSnapshot _parse(String place, Map<String, dynamic> v) {
    final current = v['current'] as Map<String, dynamic>? ?? {};
    final daily = v['daily'] as Map<String, dynamic>? ?? {};
    final times = (daily['time'] as List? ?? []).cast<String>();
    final codes = (daily['weather_code'] as List? ?? []);
    final highs = (daily['temperature_2m_max'] as List? ?? []);
    final lows = (daily['temperature_2m_min'] as List? ?? []);
    final rains = (daily['precipitation_probability_max'] as List? ?? []);
    final sunrises = (daily['sunrise'] as List? ?? []);
    final sunsets = (daily['sunset'] as List? ?? []);

    String hhmm(dynamic v) {
      final s = v as String?;
      if (s == null || s.length < 16) return '';
      return s.substring(11, 16); // timezone=auto 时已是当地时间
    }

    final days = <WeatherDay>[];
    final now = DateTime.now();
    final todayDate = DateTime(now.year, now.month, now.day);
    for (var i = 0; i < times.length && i < forecastDays; i++) {
      final dt = DateTime.tryParse(times[i]);
      String label;
      if (dt == null) {
        label = '';
      } else {
        // 用日期差判断，避免月末/年末 now.day+1 越界导致「明天」误判。
        final diff = DateTime(dt.year, dt.month, dt.day).difference(todayDate).inDays;
        if (diff == 0) {
          label = '今天';
        } else if (diff == 1) {
          label = '明天';
        } else {
          const wd = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];
          label = wd[dt.weekday - 1];
        }
      }
      days.add(WeatherDay(
        label: label,
        date: times[i],
        high: (highs[i] as num?)?.toDouble() ?? 0,
        low: (lows[i] as num?)?.toDouble() ?? 0,
        code: (codes[i] as num?)?.toInt() ?? 3,
        rain: (rains[i] as num?)?.toInt() ?? 0,
      ));
    }

    return WeatherSnapshot(
      place: place,
      temp: (current['temperature_2m'] as num?)?.toDouble() ?? 0,
      feelsLike: (current['apparent_temperature'] as num?)?.toDouble() ?? 0,
      humidity: (current['relative_humidity_2m'] as num?)?.toInt() ?? 0,
      wind: (current['wind_speed_10m'] as num?)?.toDouble() ?? 0,
      code: (current['weather_code'] as num?)?.toInt() ?? 3,
      isDay: (current['is_day'] as num?)?.toInt() == 1,
      sunrise: sunrises.isNotEmpty ? hhmm(sunrises.first) : null,
      sunset: sunsets.isNotEmpty ? hhmm(sunsets.first) : null,
      days: days,
      at: DateTime.now().millisecondsSinceEpoch ~/ 1000,
    );
  }

  Future<void> saveCache(WeatherSnapshot s) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_cacheKey, jsonEncode(s.toJson()));
  }

  Future<WeatherSnapshot?> loadCache() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_cacheKey);
    if (raw == null) return null;
    try {
      return WeatherSnapshot.fromJson(jsonDecode(raw) as Map<String, dynamic>);
    } catch (_) {
      return null;
    }
  }

  void dispose() => _client.close();
}